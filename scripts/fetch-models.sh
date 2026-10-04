#!/bin/sh
# Downloads the models Omacull uses into ~/.local/share/omacull/models,
# checking each one. Omacull never downloads anything itself.
#
#   YuNet (face_detection_yunet_2023mar.onnx), from OpenCV's model zoo, MIT:
#   face and eye detection (M5).
#
# ONNX Runtime comes from the system: on Arch, `pacman -S onnxruntime`.
set -eu

dir="${XDG_DATA_HOME:-$HOME/.local/share}/omacull/models/face-detect-yunet"
file="$dir/face_detection_yunet_2023mar.onnx"
url="https://huggingface.co/opencv/face_detection_yunet/resolve/3cc26e7f1014a5ee5d74a42acee58bafc9d0a310/face_detection_yunet_2023mar.onnx"
sha="8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4"

mkdir -p "$dir"
if [ -f "$file" ] && echo "$sha  $file" | sha256sum -c --quiet 2>/dev/null; then
    echo "YuNet is already there: $file"
    exit 0
fi
curl -fL --progress-bar -o "$file.part" "$url"
echo "$sha  $file.part" | sha256sum -c --quiet
mv "$file.part" "$file"
echo "YuNet: $file"
