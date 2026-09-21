{ lib
, stdenvNoCC
, fetchFromGitHub
, makeWrapper
, python3
, bash
, coreutils
, findutils
, gnugrep
, gnused
, git
, nodejs
, openssh
, ripgrep
, tmux
, file
}:
# Odysseus AI is not in nixpkgs (no package, no NixOS module, no open PR as of
# 2026-09-18), and upstream's GHCR package is not publicly pullable -- the
# anonymous token endpoint 401s on ghcr.io/odysseus-dev/odysseus where a public
# image such as ghcr.io/home-assistant/home-assistant returns 200. Upstream only
# documents `docker compose up --build` or a venv install, so there is no image
# to pin the way ../../nixos/romeo/services/journiv.nix does. Hence this
# derivation.
#
# It is deliberately NOT buildPythonApplication: the repo's pyproject.toml holds
# only pytest config, with no [project] table and no packaging metadata at all.
# It is a source tree you run with uvicorn, so this copies the tree into the
# store and wraps app.py with an interpreter that already has the deps.
let
  # Mirrors requirements.txt at the pinned rev, minus three entries:
  #
  #   pytest, pytest-asyncio, httpx2  -- test-only. httpx2 is in requirements.txt
  #     purely to silence a starlette.testclient deprecation warning ("prefers
  #     httpx2 since Starlette 1.2.0"); runtime code imports `httpx`, which is
  #     here. Leaving all three out keeps a second HTTP stack out of the closure.
  #
  #   chromadb-client -- not in nixpkgs. It is upstream's lightweight
  #     HTTP-only client split of chromadb, and src/chroma_client.py does a bare
  #     `import chromadb` then builds an HttpClient, so the full `chromadb`
  #     package below satisfies it. Costs closure size, not behaviour.
  #
  #   psycopg2-binary -> psycopg2. Same library; the -binary wheel exists to
  #     avoid needing libpq at build time, which is not a problem Nix has. Only
  #     reachable when DATABASE_URL points at Postgres (the default is SQLite
  #     under the data dir), but SQLAlchemy imports it inside create_engine(),
  #     so it has to be present before that is ever configured.
  pythonEnv = python3.withPackages (ps: with ps; [
    fastapi
    uvicorn
    python-multipart
    python-dotenv
    httpx
    httpcore
    pydantic
    # requirements.txt asks for >=2.14.1 and unstable is on 2.12.0. The only
    # direct use is src/config.py's BaseSettings + SettingsConfigDict, an API
    # that has been stable since 2.0, so the floor looks like upstream keeping
    # current rather than a feature they depend on. Revisit if a settings class
    # ever fails to construct at startup.
    pydantic-settings
    sqlalchemy
    pypdf
    beautifulsoup4
    charset-normalizer
    numpy
    chromadb
    fastembed
    youtube-transcript-api
    markdown
    nh3
    icalendar
    python-dateutil
    caldav
    cryptography
    bcrypt
    mcp
    pyotp
    qrcode
    pillow # qrcode[pil] extra -- 2FA setup renders its QR as a PNG
    croniter
    psycopg2

    # Not in requirements.txt: upstream installs it in the Dockerfile only,
    # because `import magic` resolves libmagic at import time and they did not
    # want to regress venv installs on hosts without the shared library. Nix
    # always has it (python-magic here propagates `file`), so the
    # content-based MIME sniffing in src/upload_handler.py works rather than
    # falling back to extension guessing.
    python-magic
  ]);

  # PATH for the wrapped process. The agent's shell tool, the Cookbook, and the
  # built-in Browser MCP server all shell out, and a systemd unit starts with
  # no useful PATH at all.
  #
  # Two things upstream's image has that are deliberately absent:
  #   chromium -- the Browser MCP server's actual browser. ~150 MiB of closure
  #     for a feature nothing here has asked for; add it to this list if that
  #     server is ever wanted.
  #   docker CLI -- the image ships it for the host-Docker overlay
  #     (ODYSSEUS_ENABLE_HOST_DOCKER). Handing an LLM agent a socket that grants
  #     root on romeo is not something to enable by accident.
  runtimePath = [
    bash
    coreutils
    findutils
    gnugrep
    gnused
    ripgrep
    git
    tmux # Cookbook runs background model downloads/serves inside tmux
    openssh # Cookbook's remote-server probes and serves
    nodejs # provides npx, which the built-in MCP servers launch
    file
  ];
in
stdenvNoCC.mkDerivation (_finalAttrs: {
  pname = "odysseus-ai";
  # APP_VERSION from src/constants.py at the pinned rev. Upstream publishes no
  # git tags, so the date suffix is what actually identifies this build.
  version = "1.0.3-unstable-2026-09-05";

  src = fetchFromGitHub {
    owner = "odysseus-dev";
    repo = "odysseus";
    # `main` is the curated branch; `dev` is upstream's default and takes the
    # newest changes first. Pin a `main` commit -- this repo is not the place to
    # ride someone else's development branch.
    rev = "934d23c0be29c9721385f34565c0ae2cbd60da04";
    hash = "sha256-/kGtXHIP9XTMqxK7aP5BotnCKY1Zc9MVUFVd5w0HdMg=";
  };

  nativeBuildInputs = [ makeWrapper ];

  dontBuild = true;
  dontConfigure = true;

  installPhase = ''
    runHook preInstall

    mkdir -p $out/share/odysseus

    # Everything the running app actually reads. Left out: tests/ (5.9M),
    # website/ (6.3M), specs/, assets/ (nothing under static/ or in any .py
    # references it -- it is README and landing-page art), docker/, swift/, and
    # the Windows/macOS launcher scripts.
    cp -r \
      app.py \
      launcher.py \
      companion \
      config \
      core \
      integrations \
      licenses \
      mcp_servers \
      routes \
      scripts \
      services \
      src \
      static \
      $out/share/odysseus/

    # setup.py is NOT installed on purpose. It is upstream's first-run wizard and
    # it writes into the app root -- os.makedirs(BASE_DIR/"logs") and a copy of
    # .env.example to BASE_DIR/.env -- which is read-only here, so its very first
    # step would fail. Nothing in it is load-bearing: core/database.py calls
    # init_db() at import time (bottom of the module), and the NixOS module does
    # the data-directory and admin-account bootstrap itself.

    makeWrapper ${pythonEnv}/bin/python $out/bin/odysseus \
      --add-flags $out/share/odysseus/app.py \
      --prefix PATH : ${lib.makeBinPath runtimePath}

    runHook postInstall
  '';

  # The module needs an interpreter with bcrypt on it to seed the admin account
  # into data/auth.json before the first start.
  passthru.python = pythonEnv;

  meta = {
    description = "Self-hosted AI workspace for chat, agents, research, documents, email, and local models";
    homepage = "https://odysseusai.dev/";
    changelog = "https://github.com/odysseus-dev/odysseus/commits/main";
    license = lib.licenses.agpl3Plus;
    platforms = lib.platforms.linux;
    mainProgram = "odysseus";
    maintainers = [ ];
  };
})
