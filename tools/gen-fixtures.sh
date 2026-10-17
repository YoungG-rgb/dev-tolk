#!/usr/bin/env bash
# Генерирует короткие тестовые файлы разных форматов (нужны ffmpeg и, на macOS, afconvert).
# Использование: [DUR=секунды] tools/gen-fixtures.sh <каталог>
# Только для разработки: ffmpeg не входит в поставку приложения (TD-07).
set -euo pipefail
out="${1:?укажите каталог для файлов}"
mkdir -p "$out"
cd "$out"

# Тон 440 Гц длительностью DUR секунд (по умолчанию 5).
dur="${DUR:-5}"
sine() { echo -f lavfi -i "sine=frequency=440:duration=$dur:sample_rate=${1:-44100}"; }
vid="-f lavfi -i testsrc=duration=$dur:size=160x120:rate=5"
ff() { ffmpeg -loglevel error -y "$@"; }

ff $(sine) -c:a libmp3lame -b:a 128k mp3_44k.mp3
ff $(sine) -c:a aac -b:a 96k aac_lc.m4a
ff $(sine) -ac 2 -c:a alac alac_stereo.m4a
ff $(sine) -c:a pcm_s16le wav_44k.wav
ff $(sine 16000) -c:a pcm_s16le wav_16k.wav
ff $(sine 8000) -c:a pcm_s16le wav_8k.wav
ff $(sine) -c:a flac flac.flac
ff $(sine) -ac 2 -c:a vorbis -strict -2 vorbis.ogg
ff $(sine) -c:a libopus -b:a 32k opus.ogg
ff $(sine) -c:a libopus opus.webm
ff $(sine) -c:a pcm_s16le pcm.mov
ff $vid $(sine) -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest video_aac.mp4
ff $vid $(sine) -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest video_aac.mov
ff $vid -c:v libx264 -pix_fmt yuv420p video_noaudio.mp4
ff $(sine) -c:a ac3 ac3.mp4
ff $vid $(sine) -c:v libx264 -pix_fmt yuv420p -c:a ac3 -shortest video_ac3.mp4
if command -v afconvert >/dev/null; then
  afconvert -f m4af -d aach -b 32000 wav_44k.wav heaac.m4a
fi
ls -l
