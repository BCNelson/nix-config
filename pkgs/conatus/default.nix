{
  lib,
  buildNpmPackage,
  fetchFromGitHub,
  fetchurl,
  geist-font,
  makeWrapper,
  nodejs_22,
}:
# Conatus — self-hosted, Todoist-style task manager. Upstream ships only docker
# images; this is the same two images built natively:
#
#   conatus      the Next.js standalone server (the `run` image)
#   conatus-ops  migrations + first-admin bootstrap (the `-ops` image), which
#                upstream deliberately keeps on its own tiny dependency set
#
# Consumed by nixos/romeo/services/conatus.nix.
let
  version = "1.0.0";

  src = fetchFromGitHub {
    owner = "nojusmorkunas";
    repo = "conatus";
    tag = "v${version}";
    hash = "sha256-0var6olZFor5CCUJMgA/7mZgGYLhRCZ4qEwUEXxcK6M=";
  };

  # app/layout.tsx pulls both typefaces through next/font/google, which
  # downloads them during `next build` — impossible in the sandbox. They are
  # swapped for next/font/local over pinned copies of the same variable fonts.
  hankenGrotesk = fetchurl {
    name = "HankenGrotesk.ttf";
    url = "https://raw.githubusercontent.com/google/fonts/e44c4b011a820c2cbe2fd2cfa8052037d7edb571/ofl/hankengrotesk/HankenGrotesk%5Bwght%5D.ttf";
    hash = "sha256-gTs/j6CWVAVmmomzjlG779le72uOINHLLYwQzOBiZi8=";
  };
  geistMono = "${geist-font}/share/fonts/truetype/GeistMono[wght].ttf";

  meta = {
    description = "Self-hosted task manager for projects, recurring work and reminders";
    homepage = "https://github.com/nojusmorkunas/conatus";
    license = lib.licenses.agpl3Plus;
    platforms = lib.platforms.linux;
  };
in {
  conatus = buildNpmPackage {
    pname = "conatus";
    inherit version src;
    nodejs = nodejs_22;

    npmDepsHash = "sha256-7LH+cLJxt7vcdVUEw5iQ2Ct2GPK0Jx/LMjnj4zBd5z8=";

    nativeBuildInputs = [makeWrapper];

    postPatch = ''
      mkdir -p app/fonts
      cp ${hankenGrotesk} app/fonts/HankenGrotesk.ttf
      cp "${geistMono}" app/fonts/GeistMono.ttf

      substituteInPlace app/layout.tsx \
        --replace-fail 'import { Geist_Mono, Hanken_Grotesk } from "next/font/google";' \
                       'import localFont from "next/font/local";' \
        --replace-fail 'Hanken_Grotesk({' 'localFont({ src: "./fonts/HankenGrotesk.ttf", weight: "100 900",' \
        --replace-fail 'Geist_Mono({' 'localFont({ src: "./fonts/GeistMono.ttf", weight: "100 900",' \
        --replace-fail 'subsets: ["latin"],' ""
    '';

    env = {
      NEXT_TELEMETRY_DISABLED = "1";
      # Same placeholders as upstream's Dockerfile: modules read these at import
      # time during page collection, but nothing connects to them.
      DATABASE_URL = "postgres://build:build@127.0.0.1:5432/build";
      S3_ENDPOINT = "127.0.0.1";
      S3_PORT = "9000";
      S3_ACCESS_KEY = "build-placeholder";
      S3_SECRET_KEY = "build-placeholder";
      S3_BUCKET = "build-placeholder";
      SMTP_HOST = "127.0.0.1";
      SMTP_PORT = "1025";
      SMTP_FROM = "build@localhost";
    };

    # The standalone output is self-contained: server.js plus only the traced
    # node_modules it needs. Static assets are not part of it and have to be
    # copied alongside, exactly as the Dockerfile does.
    installPhase = ''
      runHook preInstall

      mkdir -p $out/share/conatus $out/bin
      cp -r .next/standalone/. $out/share/conatus/
      cp -r .next/static $out/share/conatus/.next/static
      cp -r public $out/share/conatus/public

      # Next writes its runtime cache (image optimisation, fetch cache) under
      # .next/cache, which would be inside the read-only store. The service gives
      # it CacheDirectory=conatus; this points the app at it.
      ln -s /var/cache/conatus $out/share/conatus/.next/cache

      makeWrapper ${lib.getExe nodejs_22} $out/bin/conatus \
        --chdir $out/share/conatus \
        --set-default NODE_ENV production \
        --set-default NEXT_TELEMETRY_DISABLED 1 \
        --add-flags $out/share/conatus/server.js

      runHook postInstall
    '';

    meta = meta // {mainProgram = "conatus";};
  };

  conatus-ops = buildNpmPackage {
    pname = "conatus-ops";
    inherit version src;
    nodejs = nodejs_22;
    sourceRoot = "${src.name}/ops";

    npmDepsHash = "sha256-Th6NwdLVL0JN2dlgMNg9l4iLPFZ1gr0hKUvNnWVQxsA=";
    dontNpmBuild = true;

    nativeBuildInputs = [makeWrapper];

    # tsx runs the TypeScript directly and resolves the `@/…` imports through
    # tsconfig paths, so the layout has to match the image: package root with
    # lib/, scripts/ and tsconfig.json beside node_modules.
    installPhase = ''
      runHook preInstall

      root=$out/share/conatus-ops
      mkdir -p $root $out/bin
      cp -r node_modules package.json $root/
      cp -r ${src}/lib ${src}/scripts ${src}/tsconfig.json $root/

      for cmd in migrate bootstrap-admin; do
        makeWrapper $root/node_modules/.bin/tsx $out/bin/conatus-$cmd \
          --chdir $root \
          --prefix PATH : ${lib.makeBinPath [nodejs_22]} \
          --add-flags $root/scripts/$cmd.ts
      done

      runHook postInstall
    '';

    inherit meta;
  };
}
