//! Потоковая запись WAV (16 кГц, моно, 16 бит) с атомарной заменой итогового файла (TD-09).

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

use tempfile::NamedTempFile;

use crate::decode::TARGET_RATE;
use crate::error::{EngineError, Result};

const HEADER_LEN: u32 = 44;

pub struct WavWriter {
    tmp: Option<NamedTempFile>,
    out: BufWriter<File>,
    data_bytes: u64,
}

impl WavWriter {
    /// Создаёт временный файл рядом с `dest`; итоговый файл появляется только в `finish`.
    pub fn create(dest: &Path) -> Result<Self> {
        let dir = dest
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let tmp = tempfile::Builder::new()
            .prefix(".tolk-extract-")
            .suffix(".part")
            .tempfile_in(dir)
            .map_err(|e| EngineError::from_output_io(&e))?;
        let file = tmp.reopen().map_err(|e| EngineError::from_output_io(&e))?;
        let mut out = BufWriter::with_capacity(1 << 20, file);
        out.write_all(&header(0))
            .map_err(|e| EngineError::from_output_io(&e))?;
        Ok(Self {
            tmp: Some(tmp),
            out,
            data_bytes: 0,
        })
    }

    pub fn write_f32(&mut self, samples: &[f32]) -> Result<()> {
        let mut bytes = Vec::with_capacity(samples.len() * 2);
        for &s in samples {
            let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        self.out
            .write_all(&bytes)
            .map_err(|e| EngineError::from_output_io(&e))?;
        self.data_bytes += bytes.len() as u64;
        Ok(())
    }

    /// Дописывает размеры в заголовок, сбрасывает данные на диск и переименовывает файл.
    pub fn finish(mut self, dest: &Path) -> Result<()> {
        let data = u32::try_from(self.data_bytes).map_err(|_| {
            EngineError::new(
                crate::error::ErrorCode::OutputFailed,
                "Результат больше 4 ГБ",
            )
        })?;
        let io = |e: std::io::Error| EngineError::from_output_io(&e);
        self.out.flush().map_err(io)?;
        let file = self.out.get_mut();
        file.seek(SeekFrom::Start(0)).map_err(io)?;
        file.write_all(&header(data)).map_err(io)?;
        file.sync_all().map_err(io)?;
        let tmp = self.tmp.take().expect("временный файл создан в create");
        tmp.persist(dest)
            .map_err(|e| EngineError::from_output_io(&e.error))?;
        Ok(())
    }
}

fn header(data_bytes: u32) -> [u8; HEADER_LEN as usize] {
    let mut h = [0u8; HEADER_LEN as usize];
    let byte_rate = TARGET_RATE * 2;
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data_bytes).to_le_bytes());
    h[8..12].copy_from_slice(b"WAVE");
    h[12..16].copy_from_slice(b"fmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
    h[22..24].copy_from_slice(&1u16.to_le_bytes()); // моно
    h[24..28].copy_from_slice(&TARGET_RATE.to_le_bytes());
    h[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    h[32..34].copy_from_slice(&2u16.to_le_bytes()); // выравнивание блока
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    h
}
