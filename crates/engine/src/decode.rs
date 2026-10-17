//! Открытие аудио и видеофайлов, проверка дорожек и потоковое декодирование в моно 16 кГц f32.
//!
//! Видеопоток никогда не декодируется: демультиплексор читает контейнер, нужная нам
//! аудиодорожка выбирается среди остальных.

use std::fs::File;
use std::path::Path;
use std::sync::OnceLock;

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};
use symphonia::core::codecs::audio::well_known::profiles::{
    CODEC_PROFILE_AAC_HE, CODEC_PROFILE_AAC_HE_V2,
};
use symphonia::core::codecs::audio::well_known::*;
use symphonia::core::codecs::audio::{
    AudioCodecId, AudioCodecParameters, AudioDecoderOptions, CODEC_ID_NULL_AUDIO,
};
use symphonia::core::codecs::registry::CodecRegistry;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, Track, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia_adapter_libopus::OpusDecoder;

use crate::error::{EngineError, ErrorCode, Result};

/// Частота, в которую приводится весь звук (вход whisper.cpp, Silero VAD, GigaAM).
pub const TARGET_RATE: u32 = 16_000;

/// Размер блока ресемплера на входе, в кадрах.
const RESAMPLE_CHUNK: usize = 1024;

/// Допустимая доля неудачно декодированных пакетов, после которой файл считается повреждённым.
const MAX_BAD_PACKET_RATIO: f64 = 0.2;

/// Результат считается неполным, если короче заявленного больше чем на столько мс и больше чем на 3%.
const MIN_MISSING_MS: u64 = 1000;
const MAX_DECODED_RATIO: f64 = 0.97;

static CODECS: OnceLock<CodecRegistry> = OnceLock::new();

fn codecs() -> &'static CodecRegistry {
    CODECS.get_or_init(|| {
        let mut registry = CodecRegistry::new();
        symphonia::default::register_enabled_codecs(&mut registry);
        // Opus не поддерживается Symphonia напрямую (TD-07): подключаем libopus.
        registry.register_audio_decoder::<OpusDecoder>();
        registry
    })
}

/// Человекочитаемое название кодека для сообщений об ошибках.
pub fn codec_name(id: AudioCodecId) -> String {
    const NAMES: &[(AudioCodecId, &str)] = &[
        (CODEC_ID_AAC, "AAC"),
        (CODEC_ID_MP3, "MP3"),
        (CODEC_ID_MP2, "MP2"),
        (CODEC_ID_MP1, "MP1"),
        (CODEC_ID_FLAC, "FLAC"),
        (CODEC_ID_VORBIS, "Vorbis"),
        (CODEC_ID_OPUS, "Opus"),
        (CODEC_ID_ALAC, "ALAC"),
        (CODEC_ID_AC3, "AC-3"),
        (CODEC_ID_EAC3, "E-AC-3"),
        (CODEC_ID_AC4, "AC-4"),
        (CODEC_ID_DCA, "DTS"),
        (CODEC_ID_WMA, "WMA"),
        (CODEC_ID_SPEEX, "Speex"),
        (CODEC_ID_WAVPACK, "WavPack"),
        (CODEC_ID_PCM_S16LE, "PCM"),
        (CODEC_ID_PCM_S16BE, "PCM"),
        (CODEC_ID_PCM_S24LE, "PCM"),
        (CODEC_ID_PCM_S32LE, "PCM"),
        (CODEC_ID_PCM_F32LE, "PCM"),
        (CODEC_ID_PCM_F64LE, "PCM"),
        (CODEC_ID_PCM_U8, "PCM"),
        (CODEC_ID_PCM_ALAW, "A-law"),
        (CODEC_ID_PCM_MULAW, "µ-law"),
    ];
    NAMES
        .iter()
        .find(|(known, _)| *known == id)
        .map(|(_, name)| (*name).to_string())
        .unwrap_or_else(|| "неизвестный".to_string())
}

/// Сведения о выбранной аудиодорожке.
#[derive(Debug, Clone)]
pub struct MediaInfo {
    pub container: &'static str,
    pub codec: String,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub duration_ms: Option<u64>,
    pub has_video: bool,
}

/// Открытый файл с выбранной аудиодорожкой.
pub struct Source {
    format: Box<dyn FormatReader>,
    track_id: u32,
    params: AudioCodecParameters,
    pub info: MediaInfo,
}

fn map_sym_error(err: SymError) -> EngineError {
    match err {
        SymError::IoError(e) => EngineError::from_input_io(&e),
        SymError::DecodeError(_) | SymError::LimitError(_) => {
            EngineError::new(ErrorCode::CorruptFile, "Файл повреждён")
        }
        SymError::Unsupported(_) => EngineError::new(
            ErrorCode::UnsupportedFormat,
            "Формат файла не распознан или не поддерживается",
        ),
        _ => EngineError::new(ErrorCode::CorruptFile, "Файл повреждён"),
    }
}

/// Открывает файл и выбирает аудиодорожку. Декодирование не начинается.
pub fn open(path: &Path) -> Result<Source> {
    let file = File::open(path).map_err(|e| EngineError::from_input_io(&e))?;
    let meta = file
        .metadata()
        .map_err(|e| EngineError::from_input_io(&e))?;
    if !meta.is_file() {
        return Err(EngineError::new(
            ErrorCode::CorruptFile,
            "Указанный путь не является файлом",
        ));
    }
    if meta.len() == 0 {
        return Err(EngineError::new(ErrorCode::CorruptFile, "Файл пуст"));
    }

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let looks_like_media = sniff_media_signature(&file);
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| match e {
            // Известная сигнатура, но разобрать не удалось: оборванная запись (нет индекса moov и т. п.).
            SymError::Unsupported(_) if looks_like_media => EngineError::new(
                ErrorCode::CorruptFile,
                "Файл повреждён или запись не завершена: не удалось прочитать структуру файла",
            ),
            // «Формат не определён»: текст в файле .mp3, неизвестный контейнер.
            SymError::Unsupported(_) => EngineError::new(
                ErrorCode::UnsupportedFormat,
                "Формат файла не распознан: он повреждён, не является аудио или видео или не поддерживается",
            ),
            other => map_sym_error(other),
        })?;

    let container = format.format_info().short_name;
    let has_video = format
        .tracks()
        .iter()
        .any(|t| matches!(t.codec_params, Some(CodecParameters::Video(_))));

    let audio_tracks: Vec<&Track> = format
        .tracks()
        .iter()
        .filter(|t| matches!(t.codec_params, Some(CodecParameters::Audio(_))))
        .collect();

    if audio_tracks.is_empty() {
        let msg = if has_video {
            "В видеофайле нет аудиодорожки"
        } else {
            "В файле нет аудиодорожки"
        };
        return Err(EngineError::new(ErrorCode::NoAudioTrack, msg));
    }

    // Первая дорожка, для которой есть декодер; дорожка по умолчанию — в приоритете.
    let default_id = format.default_track(TrackType::Audio).map(|t| t.id);
    let supported = |t: &Track| -> bool {
        match &t.codec_params {
            Some(CodecParameters::Audio(p)) => {
                p.codec != CODEC_ID_NULL_AUDIO && codecs().get_audio_decoder(p.codec).is_some()
            }
            _ => false,
        }
    };
    let chosen = audio_tracks
        .iter()
        .find(|t| Some(t.id) == default_id && supported(t))
        .or_else(|| audio_tracks.iter().find(|t| supported(t)))
        .copied();

    let Some(track) = chosen else {
        let first = audio_tracks[0];
        let name = match &first.codec_params {
            Some(CodecParameters::Audio(p)) => codec_name(p.codec),
            _ => "неизвестный".to_string(),
        };
        return Err(EngineError::unsupported_codec(name));
    };

    let Some(CodecParameters::Audio(params)) = track.codec_params.clone() else {
        unreachable!("выбранная дорожка — аудио");
    };

    // HE-AAC: Symphonia декодирует только базовый слой (без SBR). Результат звучал бы
    // с половинной частотой и неверной длительностью, поэтому отклоняем (R-08, TD-07).
    if params.codec == CODEC_ID_AAC
        && matches!(
            params.profile,
            Some(p) if p == CODEC_PROFILE_AAC_HE || p == CODEC_PROFILE_AAC_HE_V2
        )
    {
        return Err(EngineError::unsupported_codec("HE-AAC"));
    }

    let duration_ms = track_duration_ms(track, &params);
    let info = MediaInfo {
        container,
        codec: codec_name(params.codec),
        sample_rate: params.sample_rate,
        channels: params.channels.as_ref().map(|c| c.count() as u32),
        duration_ms,
        has_video,
    };
    let track_id = track.id;

    Ok(Source {
        format,
        track_id,
        params,
        info,
    })
}

/// Начало файла похоже на известный контейнер (MP4/MOV, WAV, OGG, FLAC, MKV/WebM, ID3).
fn sniff_media_signature(mut file: &File) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let mut head = [0u8; 12];
    let n = file.read(&mut head).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(0));
    let head = &head[..n];
    head.get(4..8) == Some(b"ftyp")
        || head.starts_with(b"RIFF")
        || head.starts_with(b"OggS")
        || head.starts_with(b"fLaC")
        || head.starts_with(b"ID3")
        || head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3])
}

fn track_duration_ms(track: &Track, params: &AudioCodecParameters) -> Option<u64> {
    if let (Some(frames), Some(rate)) = (track.num_frames, params.sample_rate) {
        if rate > 0 {
            return Some(frames * 1000 / u64::from(rate));
        }
    }
    if let (Some(dur), Some(tb)) = (track.duration, track.time_base) {
        if let Some(t) = tb.calc_duration(dur) {
            return u64::try_from(t.as_millis()).ok();
        }
    }
    None
}

impl Source {
    /// Длительность в мс. Если контейнер её не сообщает (MP3 без заголовка Xing), пакеты
    /// просматриваются без декодирования.
    pub fn duration_ms_or_scan(mut self) -> Result<Option<u64>> {
        if self.info.duration_ms.is_some() {
            return Ok(self.info.duration_ms);
        }
        let time_base = self
            .format
            .tracks()
            .iter()
            .find(|t| t.id == self.track_id)
            .and_then(|t| t.time_base);
        let rate = self.params.sample_rate;
        let mut total_ticks: u128 = 0;
        loop {
            match self.format.next_packet() {
                Ok(Some(packet)) => {
                    if packet.track_id == self.track_id {
                        total_ticks += u128::from(packet.dur.get());
                    }
                }
                Ok(None) => break,
                Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(SymError::ResetRequired) => break,
                Err(e) => return Err(map_sym_error(e)),
            }
        }
        let ms = match (time_base, rate) {
            (Some(tb), _) => {
                let ns = total_ticks * 1_000_000_000 * u128::from(tb.numer.get())
                    / u128::from(tb.denom.get());
                Some((ns / 1_000_000) as u64)
            }
            (None, Some(r)) if r > 0 => Some((total_ticks * 1000 / u128::from(r)) as u64),
            _ => None,
        };
        Ok(ms)
    }
}

/// Потоковый ресемплер моно-звука в `TARGET_RATE`.
struct MonoResampler {
    inner: Option<Fft<f32>>,
    in_rate: u32,
    pending: Vec<f32>,
    out_buf: Vec<f32>,
    skip: usize,
    total_in: u64,
    emitted: u64,
}

impl MonoResampler {
    fn new(in_rate: u32) -> Result<Self> {
        let inner = if in_rate == TARGET_RATE {
            None
        } else {
            Some(
                Fft::<f32>::new(
                    in_rate as usize,
                    TARGET_RATE as usize,
                    RESAMPLE_CHUNK,
                    1,
                    FixedSync::Input,
                )
                .map_err(|e| EngineError::new(ErrorCode::Internal, format!("Ресемплер: {e}")))?,
            )
        };
        let (skip, out_len) = match &inner {
            Some(r) => (r.output_delay(), r.output_frames_max()),
            None => (0, 0),
        };
        Ok(Self {
            inner,
            in_rate,
            pending: Vec::new(),
            out_buf: vec![0.0; out_len],
            skip,
            total_in: 0,
            emitted: 0,
        })
    }

    fn expected_total(&self) -> u64 {
        (self.total_in as f64 * f64::from(TARGET_RATE) / f64::from(self.in_rate)).ceil() as u64
    }

    /// Добавляет моно-кадры на частоте входа. Готовый результат дописывается в `out`.
    fn push(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<()> {
        self.total_in += input.len() as u64;
        let Some(resampler) = self.inner.as_mut() else {
            out.extend_from_slice(input);
            self.emitted += input.len() as u64;
            return Ok(());
        };
        self.pending.extend_from_slice(input);
        let mut pos = 0;
        loop {
            let need = resampler.input_frames_next();
            if self.pending.len() - pos < need {
                break;
            }
            let chunk = &self.pending[pos..pos + need];
            let (consumed, produced) = process(resampler, chunk, &mut self.out_buf, None)?;
            pos += consumed;
            Self::emit(
                &self.out_buf[..produced],
                &mut self.skip,
                out,
                &mut self.emitted,
            );
        }
        self.pending.drain(..pos);
        Ok(())
    }

    fn emit(produced: &[f32], skip: &mut usize, out: &mut Vec<f32>, emitted: &mut u64) {
        let cut = (*skip).min(produced.len());
        *skip -= cut;
        let rest = &produced[cut..];
        out.extend_from_slice(rest);
        *emitted += rest.len() as u64;
    }

    /// Дописывает остаток и «хвост» ресемплера; лишнее после ожидаемой длины отбрасывается.
    fn finish(&mut self, out: &mut Vec<f32>) -> Result<()> {
        let expected = self.expected_total();
        let start_len = out.len();
        let before = self.emitted;
        let Some(resampler) = self.inner.as_mut() else {
            return Ok(());
        };
        let mut remaining = std::mem::take(&mut self.pending);
        let mut guard = 0;
        while self.emitted < expected && guard < 64 {
            guard += 1;
            let need = resampler.input_frames_next();
            let take = remaining.len().min(need);
            remaining.resize(need, 0.0);
            let indexing = Indexing::new().partial_len(take);
            let (_, produced) = process(
                resampler,
                &remaining[..need],
                &mut self.out_buf,
                Some(&indexing),
            )?;
            Self::emit(
                &self.out_buf[..produced],
                &mut self.skip,
                out,
                &mut self.emitted,
            );
            remaining.clear();
        }
        let excess = self.emitted.saturating_sub(expected) as usize;
        out.truncate(out.len().saturating_sub(excess));
        debug_assert!(out.len() >= start_len);
        self.emitted = before + (out.len() - start_len) as u64;
        Ok(())
    }
}

fn process(
    resampler: &mut Fft<f32>,
    input: &[f32],
    output: &mut [f32],
    indexing: Option<&Indexing>,
) -> Result<(usize, usize)> {
    let in_frames = input.len();
    let out_frames = output.len();
    let input = InterleavedSlice::new(input, 1, in_frames)
        .map_err(|e| EngineError::new(ErrorCode::Internal, format!("Буфер ресемплера: {e}")))?;
    let mut output = InterleavedSlice::new_mut(output, 1, out_frames)
        .map_err(|e| EngineError::new(ErrorCode::Internal, format!("Буфер ресемплера: {e}")))?;
    resampler
        .process_into_buffer(&input, &mut output, indexing)
        .map_err(|e| EngineError::new(ErrorCode::Internal, format!("Ресемплинг: {e}")))
}

/// Итоги декодирования.
#[derive(Debug, Clone, Copy)]
pub struct DecodeStats {
    /// Сколько кадров моно 16 кГц выдано.
    pub frames: u64,
    /// Результат заметно короче длительности, заявленной контейнером: файл оборван или частично
    /// повреждён. Выдано всё, что удалось прочитать.
    pub incomplete: bool,
    /// Число пакетов, которые не удалось декодировать и которые пропущены.
    pub skipped_packets: u64,
}

/// Декодирует файл в моно 16 кГц f32 и передаёт блоками в `sink`.
///
/// `progress` вызывается с долей 0.0..=1.0, если длительность известна.
pub fn decode_to_mono_16k(
    mut source: Source,
    sink: &mut dyn FnMut(&[f32]) -> Result<()>,
    progress: &mut dyn FnMut(f64),
) -> Result<DecodeStats> {
    let mut decoder = codecs()
        .make_audio_decoder(&source.params, &AudioDecoderOptions::default())
        .map_err(|e| match e {
            SymError::Unsupported(_) => {
                EngineError::unsupported_codec(codec_name(source.params.codec))
            }
            other => map_sym_error(other),
        })?;

    let total_ms = source.info.duration_ms;
    let mut resampler: Option<MonoResampler> = None;
    let mut interleaved: Vec<f32> = Vec::new();
    let mut mono: Vec<f32> = Vec::new();
    let mut out: Vec<f32> = Vec::new();
    let mut in_frames_total: u64 = 0;
    let mut in_rate: u32 = 0;
    let mut packets: u64 = 0;
    let mut bad_packets: u64 = 0;
    let mut truncated = false;
    let mut frames_out: u64 = 0;

    loop {
        let packet = match source.format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                truncated = true;
                break;
            }
            // Смена параметров потока посреди файла (цепочечный OGG): берём то, что уже прочитано.
            Err(SymError::ResetRequired) => break,
            Err(e) => {
                if frames_out == 0 && resampler.is_none() {
                    return Err(map_sym_error(e));
                }
                truncated = true;
                break;
            }
        };
        if packet.track_id != source.track_id {
            continue;
        }
        packets += 1;

        let buf = match decoder.decode(&packet) {
            Ok(b) => b,
            Err(SymError::DecodeError(_)) => {
                bad_packets += 1;
                continue;
            }
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                truncated = true;
                break;
            }
            Err(SymError::ResetRequired) => break,
            Err(e) => return Err(map_sym_error(e)),
        };

        let frames = buf.frames();
        if frames == 0 {
            continue;
        }
        let channels = buf.spec().channels().count().max(1);
        let rate = buf.spec().rate();

        if resampler.is_none() {
            in_rate = rate;
            resampler = Some(MonoResampler::new(rate)?);
        } else if rate != in_rate {
            return Err(EngineError::new(
                ErrorCode::UnsupportedFormat,
                "Частота дискретизации меняется внутри файла: такой файл не поддерживается",
            ));
        }

        interleaved.resize(buf.samples_interleaved(), 0.0);
        buf.copy_to_slice_interleaved(&mut interleaved[..]);

        mono.clear();
        if channels == 1 {
            mono.extend_from_slice(&interleaved);
        } else {
            let inv = 1.0 / channels as f32;
            mono.extend(
                interleaved
                    .chunks_exact(channels)
                    .map(|f| f.iter().sum::<f32>() * inv),
            );
        }
        in_frames_total += mono.len() as u64;

        out.clear();
        resampler
            .as_mut()
            .expect("создан выше")
            .push(&mono, &mut out)?;
        if !out.is_empty() {
            frames_out += out.len() as u64;
            sink(&out)?;
        }

        if let Some(total) = total_ms {
            if total > 0 && in_rate > 0 {
                let done_ms = in_frames_total * 1000 / u64::from(in_rate);
                progress((done_ms as f64 / total as f64).min(1.0));
            }
        }
    }

    let Some(mut resampler) = resampler else {
        return Err(EngineError::new(
            ErrorCode::CorruptFile,
            "Файл не содержит декодируемого звука",
        ));
    };

    if packets > 0 && (bad_packets as f64) / (packets as f64) > MAX_BAD_PACKET_RATIO {
        return Err(EngineError::new(
            ErrorCode::CorruptFile,
            "Файл повреждён: значительная часть аудиоданных не читается",
        ));
    }

    out.clear();
    resampler.finish(&mut out)?;
    if !out.is_empty() {
        frames_out += out.len() as u64;
        sink(&out)?;
    }
    if frames_out == 0 {
        return Err(EngineError::new(
            ErrorCode::CorruptFile,
            "Файл не содержит декодируемого звука",
        ));
    }
    progress(1.0);

    let decoded_ms = frames_out * 1000 / u64::from(TARGET_RATE);
    let short_of_container = total_ms.is_some_and(|total| {
        total > decoded_ms + MIN_MISSING_MS
            && (decoded_ms as f64) < total as f64 * MAX_DECODED_RATIO
    });
    Ok(DecodeStats {
        frames: frames_out,
        incomplete: truncated || short_of_container,
        skipped_packets: bad_packets,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, secs: f64, freq: f64) -> Vec<f32> {
        let n = (f64::from(rate) * secs) as usize;
        (0..n)
            .map(|i| {
                (2.0 * std::f64::consts::PI * freq * i as f64 / f64::from(rate)).sin() as f32 * 0.5
            })
            .collect()
    }

    fn resample(rate: u32, input: &[f32], chunk: usize) -> Vec<f32> {
        let mut r = MonoResampler::new(rate).unwrap();
        let mut out = Vec::new();
        for c in input.chunks(chunk) {
            r.push(c, &mut out).unwrap();
        }
        r.finish(&mut out).unwrap();
        out
    }

    #[test]
    fn resampler_keeps_length_and_pitch_for_common_rates() {
        for rate in [
            8_000, 11_025, 16_000, 22_050, 32_000, 44_100, 48_000, 96_000,
        ] {
            // Нерегулярный размер блоков, как у реальных декодеров.
            let out = resample(rate, &sine(rate, 3.0, 440.0), 1_153);
            let expected = 3 * TARGET_RATE as usize;
            assert!(
                out.len().abs_diff(expected) <= 1,
                "{rate} Гц: {} кадров вместо {expected}",
                out.len()
            );

            let crossings = out.windows(2).filter(|p| p[0] < 0.0 && p[1] >= 0.0).count();
            let freq = crossings as f64 / 3.0;
            assert!((freq - 440.0).abs() < 3.0, "{rate} Гц: частота {freq}");
            let peak = out.iter().fold(0f32, |m, s| m.max(s.abs()));
            assert!((0.45..0.55).contains(&peak), "{rate} Гц: амплитуда {peak}");
        }
    }

    #[test]
    fn resampler_handles_input_shorter_than_one_chunk() {
        let out = resample(44_100, &sine(44_100, 0.01, 440.0), 100);
        assert!(out.len().abs_diff(160) <= 1, "{}", out.len());
    }

    #[test]
    fn codec_names_are_readable() {
        assert_eq!(codec_name(CODEC_ID_AC3), "AC-3");
        assert_eq!(codec_name(CODEC_ID_OPUS), "Opus");
        assert_eq!(codec_name(CODEC_ID_NULL_AUDIO), "неизвестный");
    }
}
