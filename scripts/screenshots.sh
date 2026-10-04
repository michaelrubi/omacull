#!/bin/sh
# Takes the README's screenshots of a folder of raws.
#
#   scripts/screenshots.sh [folder] [where to put them]
#
# Without a folder, a made-up shoot is used (the `shoot` example); without
# somewhere to put them, they go in docs/screenshots.
#
# Omacull is shown on an output Hyprland makes for the purpose, which no
# screen shows, so no window moves on yours; the pointer does, to the middle
# of your screen, as each shot starts. It runs with settings, caches and a
# decision log of its own, in a temporary folder, and makes no marks: it
# changes nothing of yours, nor anything in the folder. Look at the shots
# before keeping them: a notification that pops up meanwhile is in them.
#
# Needs Hyprland (with its Lua config, as Omarchy has it), grim, jq and a
# release build (`cargo build --release`).
set -eu

cd "$(dirname "$0")/.."
omacull="$PWD/target/release/omacull"
out="${2:-$PWD/docs/screenshots}"
tmp="$(mktemp -d)"
output="omacull-screenshots"

shoot="${1:-$tmp/shoot}"
if [ -z "${1:-}" ]; then
    cargo run --release -q -p omacull-engine --example shoot --features testing -- "$shoot"
fi
# The second raw: the sharp frame of the made-up shoot's first burst.
first="$(find "$shoot" -maxdepth 1 \( -iname '*.arw' -o -iname '*.nef' -o -iname '*.cr3' -o -iname '*.raf' \) | sort | sed -n 2p)"

# The models, wherever they are: the data folder below isn't the real one.
data="${XDG_DATA_HOME:-$HOME/.local/share}"
model() {
    for app in omacull omapix; do
        [ -f "$data/$app/models/$1" ] && echo "$data/$app/models/$1" && return
    done
    echo "/nowhere"
}
yunet="$(model face-detect-yunet/face_detection_yunet_2023mar.onnx)"
landmarker="$(model face-landmarks-mediapipe/face_landmarks_detector.onnx)"

cleanup() {
    hyprctl -q output remove "$output" || true
    rm -rf "$tmp"
}
trap cleanup EXIT INT TERM

screen="$(hyprctl monitors -j | jq -r '.[] | select(.focused) | .name')"
hyprctl -q output create headless "$output"
hyprctl -q eval "hl.monitor({ output = \"$output\", mode = \"1920x1080@60\", position = \"auto\", scale = 1 })"
monitor="$(hyprctl monitors -j | jq --arg name "$output" '.[] | select(.name == $name) | .id')"
mkdir -p "$out"

# shot <name> <what to open> <commands>: run Omacull through the commands,
# give it a moment to load what it shows, and take its window. Each starts
# from Omacull's defaults.
shot() {
    env="XDG_CONFIG_HOME=$tmp/config-$1 XDG_CACHE_HOME=$tmp/cache XDG_DATA_HOME=$tmp/data"
    env="$env OMACULL_YUNET=$yunet OMACULL_LANDMARKER=$landmarker OMACULL_SCRIPT=$3"
    hyprctl -q eval "hl.exec_cmd(\"env $env $omacull '$2'\", { monitor = \"$output\", no_initial_focus = true, opaque = true })"
    # Opening there takes the pointer and the keyboard with it: back they
    # go, which keeps the pointer out of the shot too.
    sleep 1
    hyprctl -q eval "hl.dispatch(hl.dsp.focus({ monitor = \"$screen\" }))"
    sleep 4
    window="$(hyprctl clients -j | jq -r --argjson monitor "$monitor" \
        '.[] | select(.class == "omacull" and .monitor == $monitor) | "\(.pid) \(.at[0]),\(.at[1]) \(.size[0])x\(.size[1])"' | head -1)"
    if [ -z "$window" ]; then
        echo "Omacull didn't open for $1" >&2
        exit 1
    fi
    grim -t jpeg -q 90 -g "${window#* }" "$out/$1.jpg"
    kill "${window%% *}"
    sleep 1
    echo "$out/$1.jpg"
}

shot loupe "$first" "Stacking,Stacking,Signals,Histogram,FocusPoint"
shot survey "$shoot" "Stacking,Stacking,Signals,Survey"
shot summary "$shoot" "Stacking,Stacking,Histogram,Next,Next,Summary"
