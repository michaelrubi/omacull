#!/bin/sh
# Downloads the models Omacull uses into ~/.local/share/omacull/models,
# checking each one. Omacull never downloads anything itself.
#
#   YuNet (face_detection_yunet_2023mar.onnx), from OpenCV's model zoo, MIT:
#   face and eye detection (M5).
#
#   MediaPipe Face Landmarker (face_landmarks_detector.onnx), Google's,
#   exported to ONNX by senty-au, Apache-2.0: whether eyes are open (M7).
#
# Omapix uses the same two, and where it has them Omacull uses its copies:
# there's nothing to fetch.
#
# ONNX Runtime comes from the system: on Arch, `pacman -S onnxruntime`.
set -eu

models="${XDG_DATA_HOME:-$HOME/.local/share}/omacull/models"

# fetch <name> <folder> <file> <SHA-256> <URL>
fetch() {
    dir="$models/$2"
    file="$dir/$3"
    mkdir -p "$dir"
    if [ -f "$file" ] && echo "$4  $file" | sha256sum -c --quiet 2>/dev/null; then
        echo "$1 is already there: $file"
        return
    fi
    curl -fL --progress-bar -o "$file.part" "$5"
    echo "$4  $file.part" | sha256sum -c --quiet
    mv "$file.part" "$file"
    echo "$1: $file"
}

fetch YuNet face-detect-yunet face_detection_yunet_2023mar.onnx \
    8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4 \
    https://huggingface.co/opencv/face_detection_yunet/resolve/3cc26e7f1014a5ee5d74a42acee58bafc9d0a310/face_detection_yunet_2023mar.onnx
fetch "MediaPipe Face Landmarker" face-landmarks-mediapipe face_landmarks_detector.onnx \
    7d6e82dee82a1dca5fbddb282b3cc74571833a530de317fc22ae325c3358beeb \
    https://huggingface.co/senty-au/face_landmarks_detector-ONNX/resolve/337d58218b5b1cc597ca3c67360880b920f6ce7b/onnx/model.onnx
