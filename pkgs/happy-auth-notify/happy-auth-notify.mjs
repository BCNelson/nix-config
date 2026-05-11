#!/usr/bin/env node
import nacl from 'tweetnacl';
import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs';
import { join } from 'node:path';
import { homedir } from 'node:os';
import { randomBytes, randomUUID } from 'node:crypto';
import { setTimeout as sleep } from 'node:timers/promises';

const SERVER_URL = process.env.HAPPY_SERVER_URL || 'https://api.cluster-fluster.com';
const HAPPY_HOME = process.env.HAPPY_HOME_DIR || join(homedir(), '.happy');
const NTFY_BASE = process.env.NTFY_BASE || 'https://ntfy.sh';
const NTFY_TOPIC_FILE = process.env.NTFY_TOPIC_FILE;

const ACCESS_KEY_FILE = join(HAPPY_HOME, 'access.key');
const SETTINGS_FILE = join(HAPPY_HOME, 'settings.json');

if (existsSync(ACCESS_KEY_FILE)) {
  console.log('happy-auth-notify: already authenticated, nothing to do');
  process.exit(0);
}

if (!NTFY_TOPIC_FILE) {
  console.error('happy-auth-notify: NTFY_TOPIC_FILE must be set');
  process.exit(1);
}
const topic = readFileSync(NTFY_TOPIC_FILE, 'utf8').trim();
if (!topic) {
  console.error(`happy-auth-notify: topic file ${NTFY_TOPIC_FILE} is empty`);
  process.exit(1);
}

const b64 = (u8) => Buffer.from(u8).toString('base64');
const b64url = (u8) => Buffer.from(u8).toString('base64url');

function decryptBundle(bundle, recipientSecretKey) {
  const ephemeralPubKey = bundle.subarray(0, 32);
  const nonce = bundle.subarray(32, 32 + nacl.box.nonceLength);
  const encrypted = bundle.subarray(32 + nacl.box.nonceLength);
  return nacl.box.open(encrypted, nonce, ephemeralPubKey, recipientSecretKey);
}

async function postJson(url, body) {
  const res = await fetch(url, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!res.ok) {
    throw new Error(`POST ${url} -> ${res.status} ${res.statusText}`);
  }
  return res.json();
}

const keypair = nacl.box.keyPair.fromSecretKey(new Uint8Array(randomBytes(32)));
const pubKeyB64 = b64(keypair.publicKey);

await postJson(`${SERVER_URL}/v1/auth/request`, {
  publicKey: pubKeyB64,
  supportsV2: true,
});

const pairUrl = `happy://terminal?${b64url(keypair.publicKey)}`;
console.log(`Pairing URL: ${pairUrl}`);

const ntfyRes = await fetch(`${NTFY_BASE}/${topic}`, {
  method: 'POST',
  headers: {
    'Title': 'Happy auth pairing',
    'Click': pairUrl,
  },
  body: 'Tap to authorize this workstation as a happy machine',
});
if (!ntfyRes.ok) {
  console.error(`ntfy POST failed: ${ntfyRes.status} ${ntfyRes.statusText}`);
}

console.log('Waiting for authorization...');
while (true) {
  const resp = await postJson(`${SERVER_URL}/v1/auth/request`, {
    publicKey: pubKeyB64,
    supportsV2: true,
  });
  if (resp.state === 'authorized') {
    const token = resp.token;
    const bundle = Buffer.from(resp.response, 'base64');
    const decrypted = decryptBundle(bundle, keypair.secretKey);
    if (!decrypted) throw new Error('Failed to decrypt server response');

    let accessKey;
    if (decrypted.length === 32) {
      accessKey = { token, secret: b64(decrypted) };
    } else if (decrypted.length === 33 && decrypted[0] === 0) {
      const accountPubKey = decrypted.subarray(1, 33);
      const machineKey = randomBytes(32);
      accessKey = {
        token,
        encryption: {
          publicKey: b64(accountPubKey),
          machineKey: b64(machineKey),
        },
      };
    } else {
      throw new Error(`Unexpected auth payload: length=${decrypted.length} first=${decrypted[0]}`);
    }

    mkdirSync(HAPPY_HOME, { recursive: true, mode: 0o700 });
    writeFileSync(ACCESS_KEY_FILE, JSON.stringify(accessKey, null, 2), { mode: 0o600 });
    writeFileSync(SETTINGS_FILE, JSON.stringify({
      onboardingCompleted: false,
      machineId: randomUUID(),
    }, null, 2));
    console.log('happy-auth-notify: authentication successful');
    process.exit(0);
  }
  await sleep(2000);
}
