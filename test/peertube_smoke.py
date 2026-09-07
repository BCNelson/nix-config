"""Run on Romeo as root: private upload, A380 encode, playback, and cleanup.

Set SHAPE=720x1280 for portrait. Requires Python 3 and the running PeerTube unit.
This integration check creates an eight-second private video and deletes it.
"""

import atexit
import json
import os
import re
import threading
import secrets
import shlex
import shutil
import subprocess
import tempfile
import time
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import urlencode, urlsplit, urljoin
from urllib.request import Request, urlopen

service_environment = subprocess.check_output(
    ['systemctl', 'show', 'peertube', '-p', 'Environment', '--value'], text=True
)
service_path = next(value[5:] for value in shlex.split(service_environment) if value.startswith('PATH='))
ffmpeg = shutil.which('ffmpeg', path=service_path)
ffprobe = shutil.which('ffprobe', path=service_path)
assert ffmpeg and ffprobe, 'PeerTube FFmpeg tools were not found'
BASE = 'https://tube.nel.family'
token = None

def request(method, path, data=None, form=False, raw=False, content_type=None):
    headers = {}
    if token:
        headers['Authorization'] = 'Bearer ' + token
    if data is not None and not isinstance(data, bytes):
        data = (urlencode(data) if form else json.dumps(data)).encode()
        headers['Content-Type'] = 'application/x-www-form-urlencoded' if form else 'application/json'
    if content_type:
        headers['Content-Type'] = content_type
    with urlopen(Request(BASE + path, data, headers, method=method), timeout=120) as response:
        body = response.read()
    return body if raw else (json.loads(body) if body else None)

config = request('GET', '/api/v1/config')
assert config['signup']['allowed'] is False, 'Public registration is open'
client = request('GET', '/api/v1/oauth-clients/local')
token = request('POST', '/api/v1/users/token', {
    'client_id': client['client_id'], 'client_secret': client['client_secret'],
    'grant_type': 'password', 'username': 'root',
    'password': Path('/run/agenix/peertube-admin-password').read_text().strip(),
}, form=True)['access_token']
@atexit.register
def logout():
    if token:
        try:
            request('POST', '/api/v1/users/revoke-token')
        except Exception:
            print('Could not revoke the smoke-test admin session')

channel = request('GET', '/api/v1/users/me')['videoChannels'][0]['id']
for plugin in ['peertube-plugin-auth-openid-connect', 'peertube-plugin-transcoding-profile-debug']:
    result = request('GET', '/api/v1/plugins/' + plugin)
    print(plugin, result['version'], 'installed')

seen_gpu = threading.Event()
stop_monitor = threading.Event()
def monitor():
    while not stop_monitor.wait(0.05):
        for proc in Path('/proc').glob('[0-9]*/cmdline'):
            try:
                args = proc.read_bytes().split(b'\0')
                if args and args[0].endswith(b'/ffmpeg') and b'h264_vaapi' in args and b'/dev/dri/by-driver/i915-render' in args:
                    seen_gpu.set()
            except (OSError, PermissionError):
                pass
threading.Thread(target=monitor, daemon=True).start()
with tempfile.TemporaryDirectory(prefix='peertube-smoke-') as scratch:
    clip = Path(scratch) / 'test.mp4'
    subprocess.run([
        ffmpeg,
        '-nostdin', '-hide_banner', '-loglevel', 'error', '-f', 'lavfi',
        '-i', 'testsrc2=size=' + os.environ.get('SHAPE', '1280x720') + ':rate=24', '-f', 'lavfi', '-i', 'sine=frequency=440',
        '-t', '8', '-c:v', 'libx264', '-threads', '2', '-c:a', 'aac', str(clip),
    ], check=True)
    boundary = 'peertube-smoke-' + secrets.token_hex(8)
    parts = []
    for key, value in {'name': 'PeerTube deployment smoke test', 'channelId': str(channel), 'privacy': '3', 'waitTranscoding': 'true'}.items():
        parts.append(f'--{boundary}\r\nContent-Disposition: form-data; name="{key}"\r\n\r\n{value}\r\n'.encode())
    parts += [f'--{boundary}\r\nContent-Disposition: form-data; name="videofile"; filename="test.mp4"\r\nContent-Type: video/mp4\r\n\r\n'.encode(), clip.read_bytes(), f'\r\n--{boundary}--\r\n'.encode()]
    video = request('POST', '/api/v1/videos/upload', b''.join(parts), content_type='multipart/form-data; boundary=' + boundary)['video']
    video_id = video['uuid']
    try:
        for _ in range(120):
            detail = request('GET', '/api/v1/videos/' + video_id)
            if detail['state']['id'] == 1 and any(f['resolution']['id'] == 360 for p in detail['streamingPlaylists'] for f in p['files']):
                break
            time.sleep(5)
        else:
            raise RuntimeError('Transcoding did not complete in 10 minutes')
        assert detail['privacy']['id'] == 3, 'Smoke-test video is not private'
        playlists = detail['streamingPlaylists']
        assert playlists and playlists[0]['files'], 'No HLS renditions generated'
        file_token = request('POST', '/api/v1/videos/' + video_id + '/token', {})['files']['token']
        playlist_path = urlsplit(playlists[0]['playlistUrl']).path
        manifest = request('GET', playlist_path + '?' + urlencode({'videoFileToken': file_token}), raw=True)
        assert manifest.startswith(b'#EXTM3U'), 'HLS manifest is invalid'
        rendition = next(line for line in manifest.decode().splitlines() if line and not line.startswith('#'))
        rendition_url = urljoin(playlists[0]['playlistUrl'], rendition)
        rendition_data = request('GET', urlsplit(rendition_url).path + '?' + urlencode({'videoFileToken': file_token}), raw=True)
        media_name = re.search(r'#EXT-X-MAP:URI="([^"]+)"', rendition_data.decode())[1]
        media_path = urlsplit(urljoin(rendition_url, media_name)).path
        media = request('GET', media_path + '?' + urlencode({'videoFileToken': file_token}), raw=True)
        downloaded = Path(scratch) / 'downloaded.mp4'
        downloaded.write_bytes(media)
        probe = subprocess.run([ffprobe, '-v', 'error', '-show_entries', 'stream=codec_name,width,height', '-of', 'json', str(downloaded)], stdout=subprocess.PIPE, check=True)
        streams = json.loads(probe.stdout)['streams']
        assert any(stream['codec_name'] == 'h264' for stream in streams)
        assert seen_gpu.is_set(), 'Did not observe PeerTube FFmpeg using A380 VAAPI'
        print('Observed PeerTube FFmpeg using h264_vaapi on the A380')
        print('Downloaded HLS media validates:', streams)
        print('Private upload, transcoding, and authenticated HLS media passed')
        print('Renditions:', [f['resolution']['id'] for f in playlists[0]['files']])
        saved_token, token = token, None
        try:
            request('GET', '/api/v1/videos/' + video_id)
            raise RuntimeError('Private video was accessible anonymously')
        except HTTPError as error:
            if error.code not in (401, 403, 404):
                raise
            error.close()
            print('Anonymous access to private video denied')
        finally:
            token = saved_token
    finally:
        stop_monitor.set()
        request('DELETE', '/api/v1/videos/' + video_id)
        print('Smoke-test video removed')
