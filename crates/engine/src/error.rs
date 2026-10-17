//! Ошибки движка и их коды. Коды стабильны: на них опирается сопоставление сообщений в приложении (MVP-018, MVP-025).

use std::fmt;
use std::io;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// Неверные аргументы командной строки.
    Usage,
    /// Входной файл не найден.
    FileNotFound,
    /// Нет доступа к входному или выходному файлу.
    AccessDenied,
    /// Контейнер не распознан или не поддерживается.
    UnsupportedFormat,
    /// Кодек аудиодорожки не поддерживается (в `codec` — его название).
    UnsupportedCodec,
    /// В файле нет аудиодорожки.
    NoAudioTrack,
    /// Файл повреждён, пуст или не является медиафайлом.
    CorruptFile,
    /// Не удалось записать результат (нет места, нет доступа к каталогу).
    OutputFailed,
    /// Внутренняя ошибка движка (включая panic).
    Internal,
}

impl ErrorCode {
    pub fn exit_code(self) -> i32 {
        match self {
            ErrorCode::Internal => 1,
            ErrorCode::Usage => 2,
            ErrorCode::OutputFailed => 11,
            ErrorCode::FileNotFound
            | ErrorCode::AccessDenied
            | ErrorCode::UnsupportedFormat
            | ErrorCode::UnsupportedCodec
            | ErrorCode::NoAudioTrack
            | ErrorCode::CorruptFile => 10,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EngineError {
    pub code: ErrorCode,
    pub message: String,
    pub codec: Option<String>,
}

impl EngineError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            codec: None,
        }
    }

    pub fn unsupported_codec(codec: impl Into<String>) -> Self {
        let codec = codec.into();
        Self {
            code: ErrorCode::UnsupportedCodec,
            message: format!("Кодек аудио не поддерживается: {codec}"),
            codec: Some(codec),
        }
    }

    /// Ошибка чтения входного файла.
    pub fn from_input_io(err: &io::Error) -> Self {
        match err.kind() {
            io::ErrorKind::NotFound => Self::new(ErrorCode::FileNotFound, "Файл не найден"),
            io::ErrorKind::PermissionDenied => {
                Self::new(ErrorCode::AccessDenied, "Нет доступа к файлу")
            }
            _ => Self::new(
                ErrorCode::CorruptFile,
                format!("Не удалось прочитать файл: {err}"),
            ),
        }
    }

    /// Ошибка записи результата.
    pub fn from_output_io(err: &io::Error) -> Self {
        match err.kind() {
            io::ErrorKind::PermissionDenied => {
                Self::new(ErrorCode::AccessDenied, "Нет доступа для записи результата")
            }
            _ => Self::new(
                ErrorCode::OutputFailed,
                format!("Не удалось записать результат: {err}"),
            ),
        }
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for EngineError {}

pub type Result<T> = std::result::Result<T, EngineError>;
