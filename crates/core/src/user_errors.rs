//! Каталог ошибок для пользователя: код → сообщение на русском → рекомендуемое действие (MVP-025).
//!
//! Пользователь не видит текстов Rust-ошибок, путей и трассировок: технические подробности лежат в
//! `UserMessage::technical` и предназначены только для журнала. Сопоставления написаны без
//! подстановочных веток: новый вариант ошибки не компилируется, пока для него нет сообщения.

use crate::download::DownloadError;
use crate::error::StoreError;

/// Код задания, прерванного закрытием или падением приложения (`queue::ERROR_INTERRUPTED`).
pub const CODE_INTERRUPTED: &str = crate::queue::ERROR_INTERRUPTED;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserMessage {
    /// Стабильный код (для журнала, тестов и сопоставления в интерфейсе).
    pub code: String,
    /// Что произошло, понятным языком.
    pub message: String,
    /// Что сделать пользователю.
    pub action: String,
    /// Подробности для журнала. В интерфейсе не показываются.
    pub technical: Option<String>,
}

fn msg(code: &str, message: impl Into<String>, action: &str) -> UserMessage {
    UserMessage {
        code: code.to_string(),
        message: message.into(),
        action: action.to_string(),
        technical: None,
    }
}

fn with_technical(mut m: UserMessage, technical: impl ToString) -> UserMessage {
    m.technical = Some(technical.to_string());
    m
}

const ACTION_REPORT: &str = "Повторите действие. Если ошибка повторяется, откройте папку журналов и передайте файлы разработчикам.";
const ACTION_IMPORT: &str = "Проверьте подключение к сети или импортируйте модель из файла.";
const ACTION_FREE_SPACE: &str = "Освободите место на диске и повторите.";
const ACTION_CLOSE_PROGRAM: &str = "Закройте программу, которая использует файл, и повторите.";

/// Коды ошибок движка (`docs/ENGINE_PROTOCOL.md`). `codec` — название кодека для `UNSUPPORTED_CODEC`.
pub fn from_engine(code: &str, codec: Option<&str>) -> UserMessage {
    match code {
        "USAGE" => msg(code, "Внутренняя ошибка приложения: движок вызван неверно.", ACTION_REPORT),
        "FILE_NOT_FOUND" => msg(code, "Файл не найден.", "Проверьте, что файл не перемещён и не удалён, и добавьте его снова."),
        "ACCESS_DENIED" => msg(code, "Нет доступа к файлу.", "Закройте программу, которая использует файл, или проверьте права доступа, и повторите."),
        "UNSUPPORTED_FORMAT" => msg(
            code,
            "Формат файла не распознан или не поддерживается.",
            "Поддерживаются MP3, M4A, WAV, FLAC, OGG, Opus и видео MP4/MOV. Преобразуйте запись в один из этих форматов.",
        ),
        "UNSUPPORTED_CODEC" => msg(
            code,
            match codec {
                Some(name) if !name.is_empty() => format!("Кодек аудио не поддерживается: {name}."),
                _ => "Кодек аудио не поддерживается.".to_string(),
            },
            "Преобразуйте запись в MP3, M4A (AAC) или WAV и добавьте её снова.",
        ),
        "NO_AUDIO_TRACK" => msg(code, "В файле нет аудиодорожки.", "Выберите другой файл."),
        "CORRUPT_FILE" => msg(code, "Файл повреждён, пуст или записан не до конца.", "Скачайте или запишите файл заново."),
        "OUTPUT_FAILED" => msg(code, "Не удалось сохранить результат на диск.", ACTION_FREE_SPACE),
        "INTERNAL" => msg(code, "Во время обработки произошла внутренняя ошибка.", ACTION_REPORT),
        c if c == CODE_INTERRUPTED => msg(code, "Обработка прервана.", "Нажмите «Повторить»: запись будет обработана заново."),
        // Незнакомый код (например, от более новой версии движка): код не показывается, он попадает в журнал.
        other => with_technical(msg("INTERNAL", "Во время обработки произошла внутренняя ошибка.", ACTION_REPORT), format!("код движка: {other}")),
    }
}

/// Ошибки загрузки и импорта моделей.
pub fn from_download(err: &DownloadError) -> UserMessage {
    use DownloadError::*;
    match err {
        NoConnection => msg("NET_NO_CONNECTION", err.to_string(), ACTION_IMPORT),
        Dns(host) => with_technical(msg("NET_DNS", "Не удалось найти адрес сервера: домен недоступен.", ACTION_IMPORT), format!("DNS: {host}")),
        Tls => msg("NET_TLS", err.to_string(), "Возможно, сеть подменяет сертификаты или часы компьютера неверны. Проверьте дату и время или импортируйте модель из файла."),
        Timeout => msg("NET_TIMEOUT", err.to_string(), ACTION_IMPORT),
        Interrupted => msg("DOWNLOAD_INTERRUPTED", err.to_string(), "Нажмите «Повторить»: загрузка продолжится с места остановки."),
        Forbidden => msg("NET_FORBIDDEN", err.to_string(), "Возможно, домен блокируется сетью организации. Обратитесь в ИТ-отдел или импортируйте модель из файла."),
        ProxyAuthRequired => msg("NET_PROXY_AUTH", err.to_string(), "Обратитесь в ИТ-отдел за доступом или импортируйте модель из файла."),
        NotFound => msg("MODEL_NOT_FOUND_REMOTE", err.to_string(), "Обновите приложение или импортируйте модель из файла."),
        RateLimited => msg("NET_RATE_LIMIT", err.to_string(), "Повторите загрузку позже или импортируйте модель из файла."),
        Server(code) => with_technical(msg("NET_SERVER", "Сервер загрузки временно недоступен.", "Повторите загрузку позже или импортируйте модель из файла."), format!("HTTP {code}")),
        UnexpectedStatus(code) => with_technical(msg("NET_UNEXPECTED", "Сервер ответил неожиданным образом.", "Повторите загрузку или импортируйте модель из файла."), format!("HTTP {code}")),
        BadResponse(what) => with_technical(msg("NET_BAD_RESPONSE", "Сервер вернул некорректный ответ.", "Повторите загрузку или импортируйте модель из файла."), *what),
        UntrustedRedirect(url) => with_technical(msg("NET_UNTRUSTED_REDIRECT", "Загрузка остановлена: сервер перенаправил запрос на недоверенный адрес.", ACTION_IMPORT), url),
        NoSpace { .. } => msg("NO_SPACE", err.to_string(), ACTION_FREE_SPACE),
        SizeMismatch { .. } => msg("SIZE_MISMATCH", "Файл модели получен не полностью.", "Повторите загрузку."),
        HashMismatch => msg("HASH_MISMATCH", err.to_string(), "Повторите загрузку или импортируйте модель из проверенного источника."),
        NotACatalogModel => msg("NOT_CATALOG_MODEL", err.to_string(), "Выберите файл модели, скачанный из источника, указанного в приложении."),
        Cancelled => msg("CANCELLED", err.to_string(), "Нажмите «Повторить», чтобы продолжить загрузку."),
        Io(e) => with_technical(msg("FILE_IO", "Не удалось записать файл модели.", "Проверьте свободное место и права доступа к каталогу данных."), e),
    }
}

/// Ошибки ядра: база данных, файлы заданий, словарь, модели, экспорт.
pub fn from_store(err: &StoreError) -> UserMessage {
    use StoreError::*;
    match err {
        Db(e) => with_technical(
            msg("DB_ERROR", "Ошибка локальной базы данных.", ACTION_REPORT),
            e,
        ),
        Io(e) => with_technical(
            msg(
                "FILE_IO",
                "Ошибка при работе с файлом.",
                "Проверьте свободное место и права доступа к каталогу данных.",
            ),
            e,
        ),
        SchemaTooNew { found, supported } => with_technical(
            msg(
                "DB_TOO_NEW",
                "Данные созданы более новой версией приложения.",
                "Обновите приложение до последней версии.",
            ),
            format!("версия схемы {found}, поддерживается {supported}"),
        ),
        BadMigrations(what) => with_technical(
            msg("DB_ERROR", "Ошибка локальной базы данных.", ACTION_REPORT),
            *what,
        ),
        NoSpace => msg(
            "NO_SPACE",
            "Недостаточно места на диске.",
            ACTION_FREE_SPACE,
        ),
        FileBusy(path) => with_technical(
            msg(
                "FILE_BUSY",
                "Файл занят другой программой.",
                ACTION_CLOSE_PROGRAM,
            ),
            path,
        ),
        InvalidId(what) => with_technical(
            msg("INTERNAL", "Внутренняя ошибка приложения.", ACTION_REPORT),
            what,
        ),
        BadTranscript(why) => with_technical(
            msg(
                "TRANSCRIPT_DAMAGED",
                "Файл транскрипта повреждён.",
                "Верните исходный текст или удалите запись и обработайте её заново.",
            ),
            why,
        ),
        Export(why) => with_technical(
            msg(
                "EXPORT_FAILED",
                "Не удалось создать документ.",
                "Повторите экспорт или выберите другую папку.",
            ),
            why,
        ),
        GlossaryInvalid(text) => msg(
            "GLOSSARY_INVALID",
            text.clone(),
            "Исправьте данные и повторите.",
        ),
        GlossaryDuplicate(_) => msg(
            "GLOSSARY_DUPLICATE",
            err.to_string(),
            "Измените существующий термин вместо добавления нового.",
        ),
        InvalidInput(text) => msg(
            "INVALID_INPUT",
            text.clone(),
            "Исправьте значение и повторите.",
        ),
        GlossaryNotFound => msg(
            "GLOSSARY_NOT_FOUND",
            "Термин не найден: возможно, его уже удалили.",
            "Обновите список.",
        ),
        JobNotFound(_) => msg(
            "JOB_NOT_FOUND",
            "Запись не найдена: возможно, её уже удалили.",
            "Обновите список.",
        ),
        InvalidTransition { id, from, to } => with_technical(
            msg(
                "JOB_STATE",
                "Это действие недоступно для записи в её текущем состоянии.",
                "Обновите список и повторите.",
            ),
            format!("задание {id}: {from} -> {to}"),
        ),
        UnknownModel(_) => msg(
            "MODEL_UNKNOWN",
            "Такой модели нет в каталоге приложения.",
            "Выберите модель из списка.",
        ),
        ModelNotInstalled(_) => msg(
            "MODEL_NOT_INSTALLED",
            "Модель не установлена.",
            "Установите модель на экране «Модели».",
        ),
        ModelInUse(_) => msg(
            "MODEL_IN_USE",
            "Модель сейчас используется: идёт обработка записи.",
            "Дождитесь окончания обработки или отмените её.",
        ),
        UnknownStatus(s) => with_technical(
            msg("INTERNAL", "Внутренняя ошибка приложения.", ACTION_REPORT),
            s,
        ),
    }
}
