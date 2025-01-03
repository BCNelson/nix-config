{ writeShellApplication, ffmpeg, jq, atomicparsley, coreutils }: writeShellApplication {
    name = "m4b-extractor";
    runtimeInputs = [ ffmpeg jq atomicparsley coreutils ];
    text = builtins.readFile ./m4b-chapter-extractor.sh;
}
