{ writeShellApplication, ffmpeg, jq, atomicparsley, coreutils, gnugrep }: writeShellApplication {
    name = "m4b-extractor";
    runtimeInputs = [ ffmpeg jq atomicparsley coreutils gnugrep ];
    text = builtins.readFile ./m4b-chapter-extractor.sh;
}
