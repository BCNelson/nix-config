#!/usr/bin/env bash
set -u

# script to convert m4b (audiobook) files with embedded chapted (for eg. converted from Audbile) into individual chapter files

# required: ffmpeg; jq (json interpreter) & AtomicParsley (to embed pictures and add additional metadata to m4a/m4b AAC files)

# discover the file type (extension) of the input file
if [ -z "${1+x}" ]; then
  echo "No input file provided."
  exit 1
fi

if [ -z "${1+x}" ]; then
  echo "No input file provided."
  exit 1
fi 

ext=${1##*.}
echo "extension: $ext"
# all files / folders are named based on the "shortname" of the input file
shortname=$(basename "$1" ".$ext")
chapterdata=$shortname.dat
metadata=$shortname.tmp
echo "shortname: $shortname"

extension="${1##*.}"

forcemp3=0

if [ "$extension" == "mp3" ]; then
  forcemp3=1
fi

# if an output type has been given on the command line, set parameters (used in ffmpeg command later)
if [[  ${2+x} = "mp3"  ||  $forcemp3 = 1  ]] ; then
  outputtype="mp3"
  codec="libmp3lame"
  echo mp3
elif [[ ${2+x} = "m4a" ]]; then
  outputtype="m4a"
  codec="copy"
else
  outputtype="m4b"
  codec="copy"
fi
echo "outputtype: |$outputtype|"

# if it doesn't already exist, create a json file containing the chapter breaks (you can edit this file if you want chapters to be named rather than simply "Chapter 1", etc that Audible use)
[ ! -e "$chapterdata" ] && ffprobe -loglevel error \
            -i "$1" -print_format json -show_chapters -loglevel error -sexagesimal \
            >"$chapterdata"
read -rp "Now edit the file $chapterdata if required. Press ENTER to continue."
# comment out above if you don't want the script to pause!

# read the chapters into arrays for later processing
readarray -t id <<< "$(jq -r '.chapters[].id' "$chapterdata")"
readarray -t start <<< "$(jq -r '.chapters[].start_time' "$chapterdata")"
readarray -t end <<< "$(jq -r '.chapters[].end_time' "$chapterdata")"
readarray -t title <<< "$(jq -r '.chapters[].tags.title' "$chapterdata")"

rm "$chapterdata"

# create a ffmpeg metadata file to extract addition metadata lost in splitting files - deleted afterwards
ffmpeg -loglevel error -i "$1" -f ffmetadata "$metadata"
echo "Reading metadata from $metadata"
artist_sort=$(grep -m 1 ^sort_artist "$metadata" || grep -m 1 ^artist "$metadata")
artist_sort=${artist_sort#*=}
echo "artist_sort: $artist_sort"
album_sort=$(grep -m 1 ^sort_album "$metadata" || grep -m 1 ^title "$metadata")
album_sort=${album_sort#*=}
echo "album_sort: $album_sort"
echo "Deleting $metadata"
rm "$metadata"

# create directory for the output
echo "Creating directory $shortname"
mkdir -p "$shortname"
echo -e "\fID\tStart Time\tEnd Time\tTitle\t\tFilename"
for i in "${!id[@]}"; do
  trackno=$((i+1))
  # set the name for output - currently in format <bookname>/<tranck number>
  outname="$shortname/$(printf "%02d" "$trackno"). $shortname - ${title[$i]}.$outputtype"
  #outname=$(sed -e 's/[^A-Za-z0-9._- ]/_/g' <<< $outname)
  outname="${outname//:/_}"
  echo -e "${id[$i]}\t${start[$i]}\t${end[$i]}\t${title[$i]}\n\t\t$(basename "$outname")"
  ffmpeg -loglevel error -i "$1" -vn -c $codec \
            -ss "${start[$i]}" -to "${end[$i]}" \
            -metadata title="${title[$i]}" \
            -metadata track="$trackno" \
            -map_metadata 0 -id3v2_version 3 \
            "$outname"
done


