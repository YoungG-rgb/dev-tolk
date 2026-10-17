//! Загрузка моделей и импорт из файла (TD-13, MVP-023).
//!
//! Принципы:
//! - скачивается только запись каталога, адрес приходит из встроенного каталога; редиректы разрешены
//!   только на доверенные домены и только по HTTPS (в тестах — на локальный сервер);
//! - загрузка идёт в `<файл>.part` отдельными запросами `Range` по `chunk_bytes`: каждая попытка
//!   начинается с исходного адреса каталога (подписанные ссылки CDN истекают), ограничение времени на
//!   запрос защищает от зависшего соединения, а отмена срабатывает между блоками;
//! - SHA-256 считается потоком; итоговый файл появляется только после проверки размера и суммы,
//!   подменённый или усечённый файл удаляется и не устанавливается;
//! - прерванная загрузка оставляет `.part`, «Повторить» продолжает с места остановки;
//! - сетевых вызовов вне этого модуля нет, автоматической и повторной загрузки нет.
//!
//! Сетевой клиент — `reqwest` (blocking, `native-tls`, `system-proxy`): системное хранилище сертификатов
//! и прокси из переменных окружения и из настроек ОС (ручной прокси; PAC и WPAD не читаются).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::blocking::Client;
use reqwest::header::{CONTENT_RANGE, RANGE};
use reqwest::redirect::Policy;
use reqwest::{StatusCode, Url};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::disk;
use crate::models::{catalog, ModelEntry};

const READ_BLOCK: usize = 64 * 1024;
/// Запас места сверх размера модели.
const SPACE_MARGIN: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct DownloadConfig {
    /// Разрешённые домены (сам домен и поддомены) для адреса и редиректов.
    pub allowed_hosts: Vec<String>,
    /// Запретить всё, кроме HTTPS.
    pub https_only: bool,
    /// Использовать прокси из окружения и настроек ОС.
    pub system_proxy: bool,
    pub connect_timeout: Duration,
    /// Предельное время одного запроса (одного блока).
    pub request_timeout: Duration,
    /// Размер блока `Range`.
    pub chunk_bytes: u64,
}

impl DownloadConfig {
    /// Настройки приложения: только Hugging Face и его CDN, только HTTPS, системный прокси.
    pub fn production() -> Self {
        Self {
            allowed_hosts: vec!["huggingface.co".into(), "hf.co".into()],
            https_only: true,
            system_proxy: true,
            connect_timeout: Duration::from_secs(15),
            request_timeout: Duration::from_secs(120),
            chunk_bytes: 4 * 1024 * 1024,
        }
    }
}

/// Прогресс загрузки.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Progress {
    pub downloaded: u64,
    pub total: u64,
    /// Средняя скорость текущей попытки, байт/с (0, пока данных мало).
    pub bytes_per_sec: f64,
}

/// Отмена из другого потока. Недокачанный `.part` сохраняется для продолжения.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("Нет соединения с сетью или сервером")]
    NoConnection,
    #[error("Не удалось найти адрес сервера «{0}»: домен недоступен")]
    Dns(String),
    #[error("Не удалось установить защищённое соединение (ошибка сертификата или TLS)")]
    Tls,
    #[error("Сервер не ответил вовремя")]
    Timeout,
    #[error("Соединение прервано; загрузку можно продолжить")]
    Interrupted,
    #[error("Доступ к файлу запрещён сервером или сетью (HTTP 403)")]
    Forbidden,
    #[error("Для доступа в сеть нужна авторизация на прокси-сервере (HTTP 407)")]
    ProxyAuthRequired,
    #[error("Файл модели не найден на сервере (HTTP 404)")]
    NotFound,
    #[error("Сервер временно ограничил загрузку (HTTP 429)")]
    RateLimited,
    #[error("Сервер недоступен (HTTP {0})")]
    Server(u16),
    #[error("Неожиданный ответ сервера (HTTP {0})")]
    UnexpectedStatus(u16),
    #[error("Некорректный ответ сервера: {0}")]
    BadResponse(&'static str),
    #[error("Редирект на недоверенный адрес отклонён: {0}")]
    UntrustedRedirect(String),
    #[error("Недостаточно места на диске: нужно около {} МБ, доступно {} МБ", .needed / 1_000_000, .available / 1_000_000)]
    NoSpace { needed: u64, available: u64 },
    #[error("Размер файла не совпадает с каталогом: получено {got} байт, ожидалось {expected}")]
    SizeMismatch { got: u64, expected: u64 },
    #[error("Контрольная сумма SHA-256 не совпала: файл повреждён или подменён и не установлен")]
    HashMismatch,
    #[error("Файл не является моделью из каталога приложения")]
    NotACatalogModel,
    #[error("Загрузка отменена")]
    Cancelled,
    #[error("Ошибка работы с файлом: {0}")]
    Io(#[from] io::Error),
}

impl DownloadError {
    /// Стоит ли предложить пользователю импорт модели из файла (сетевые причины).
    pub fn suggest_import(&self) -> bool {
        matches!(
            self,
            Self::NoConnection
                | Self::Dns(_)
                | Self::Tls
                | Self::Timeout
                | Self::Forbidden
                | Self::ProxyAuthRequired
                | Self::RateLimited
                | Self::Server(_)
                | Self::UntrustedRedirect(_)
                | Self::NotFound
                | Self::UnexpectedStatus(_)
                | Self::BadResponse(_)
                | Self::HashMismatch
        )
    }

    /// Можно ли продолжить загрузку кнопкой «Повторить», не меняя ничего.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::NoConnection
                | Self::Timeout
                | Self::Interrupted
                | Self::RateLimited
                | Self::Server(_)
                | Self::Cancelled
        )
    }
}

type Result<T> = std::result::Result<T, DownloadError>;

/// Классификация ошибки `reqwest` по цепочке причин.
fn classify(err: &reqwest::Error, host: &str) -> DownloadError {
    let mut chain = String::new();
    let mut has_io_error = false;
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(e) = source {
        chain.push_str(&e.to_string().to_lowercase());
        chain.push(' ');
        has_io_error |= e.downcast_ref::<io::Error>().is_some();
        source = e.source();
    }
    if err.is_redirect() {
        return DownloadError::UntrustedRedirect(
            err.url().map(|u| u.to_string()).unwrap_or_default(),
        );
    }
    if err.is_timeout() || chain.contains("timed out") {
        return DownloadError::Timeout;
    }
    if chain.contains("dns error")
        || chain.contains("failed to lookup")
        || chain.contains("nodename nor servname")
        || chain.contains("no such host")
    {
        return DownloadError::Dns(host.to_string());
    }
    if chain.contains("certificate")
        || chain.contains("tls")
        || chain.contains("ssl")
        || chain.contains("handshake")
    {
        return DownloadError::Tls;
    }
    if err.is_connect() {
        // Ошибка установки соединения без системной ошибки ввода-вывода внутри — это отказ TLS
        // (SecureTransport и Schannel формулируют его по-разному: «record overflow», «bad record mac» и т. п.).
        return if has_io_error {
            DownloadError::NoConnection
        } else {
            DownloadError::Tls
        };
    }
    if err.is_body() || err.is_decode() {
        return DownloadError::Interrupted;
    }
    DownloadError::NoConnection
}

fn status_error(status: StatusCode) -> DownloadError {
    match status.as_u16() {
        403 => DownloadError::Forbidden,
        404 => DownloadError::NotFound,
        407 => DownloadError::ProxyAuthRequired,
        429 => DownloadError::RateLimited,
        500..=599 => DownloadError::Server(status.as_u16()),
        other => DownloadError::UnexpectedStatus(other),
    }
}

fn host_allowed(url: &Url, cfg: &DownloadConfig) -> bool {
    if cfg.https_only && url.scheme() != "https" {
        return false;
    }
    if !matches!(url.scheme(), "https" | "http") {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    cfg.allowed_hosts
        .iter()
        .any(|allowed| host == allowed || host.ends_with(&format!(".{allowed}")))
}

fn build_client(cfg: &DownloadConfig) -> Result<Client> {
    let policy_cfg = cfg.clone();
    let mut builder = Client::builder()
        .user_agent(concat!("dev-tolk/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(cfg.connect_timeout)
        .timeout(cfg.request_timeout)
        .redirect(Policy::custom(move |attempt| {
            if attempt.previous().len() >= 5 {
                attempt.error("слишком много перенаправлений")
            } else if host_allowed(attempt.url(), &policy_cfg) {
                attempt.follow()
            } else {
                let target = attempt.url().to_string();
                attempt.error(format!("redirect to untrusted address: {target}"))
            }
        }));
    if !cfg.system_proxy {
        builder = builder.no_proxy();
    }
    builder.build().map_err(|e| classify(&e, ""))
}

/// Путь итогового файла модели.
pub fn model_path(entry: &ModelEntry, models_dir: &Path) -> PathBuf {
    models_dir.join(&entry.file)
}

fn part_path(entry: &ModelEntry, models_dir: &Path) -> PathBuf {
    models_dir.join(format!("{}.part", entry.file))
}

/// Модель установлена: файл на месте и его размер равен каталожному. Итоговый файл появляется
/// только после проверки SHA-256, поэтому проверять сумму при каждом запуске не нужно.
pub fn is_installed(entry: &ModelEntry, models_dir: &Path) -> bool {
    fs::metadata(model_path(entry, models_dir)).is_ok_and(|m| m.is_file() && m.len() == entry.size)
}

/// Размер недокачанной части, если она есть.
pub fn partial_bytes(entry: &ModelEntry, models_dir: &Path) -> u64 {
    fs::metadata(part_path(entry, models_dir))
        .map(|m| m.len())
        .unwrap_or(0)
}

fn check_space(models_dir: &Path, needed: u64) -> Result<()> {
    let required = needed.saturating_add(SPACE_MARGIN);
    match disk::available_bytes(models_dir) {
        Ok(available) if available < required => Err(DownloadError::NoSpace {
            needed: required,
            available,
        }),
        _ => Ok(()), // не смогли узнать — не блокируем, ошибка записи всё равно будет понятной
    }
}

fn map_write(err: io::Error) -> DownloadError {
    if err.kind() == io::ErrorKind::StorageFull {
        DownloadError::NoSpace {
            needed: 0,
            available: 0,
        }
    } else {
        DownloadError::Io(err)
    }
}

/// SHA-256 уже скачанной части файла (при продолжении).
fn hash_prefix(path: &Path, cancel: &CancelToken) -> Result<Sha256> {
    let mut hasher = Sha256::new();
    let mut file = File::open(path)?;
    let mut buf = vec![0u8; READ_BLOCK * 4];
    loop {
        if cancel.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }
        let n = file.read(&mut buf)?;
        if n == 0 {
            return Ok(hasher);
        }
        hasher.update(&buf[..n]);
    }
}

struct Speed {
    started: Instant,
    start_bytes: u64,
}

impl Speed {
    fn bytes_per_sec(&self, downloaded: u64) -> f64 {
        let secs = self.started.elapsed().as_secs_f64();
        if secs < 0.5 {
            0.0
        } else {
            (downloaded - self.start_bytes) as f64 / secs
        }
    }
}

/// Скачивает модель в `models_dir`. Возвращает путь к установленному файлу.
///
/// Уже установленная модель не скачивается повторно. Недокачанный `.part` продолжается.
pub fn download(
    entry: &ModelEntry,
    models_dir: &Path,
    cfg: &DownloadConfig,
    cancel: &CancelToken,
    on_progress: &mut dyn FnMut(&Progress),
) -> Result<PathBuf> {
    let final_path = model_path(entry, models_dir);
    if is_installed(entry, models_dir) {
        return Ok(final_path);
    }
    let url = Url::parse(&entry.url)
        .map_err(|_| DownloadError::BadResponse("некорректный адрес в каталоге"))?;
    if !host_allowed(&url, cfg) {
        return Err(DownloadError::UntrustedRedirect(entry.url.clone()));
    }
    fs::create_dir_all(models_dir)?;
    let part = part_path(entry, models_dir);

    let mut pos = partial_bytes(entry, models_dir);
    if pos > entry.size {
        fs::remove_file(&part)?;
        pos = 0;
    }
    check_space(models_dir, entry.size - pos)?;
    let mut hasher = if pos > 0 {
        hash_prefix(&part, cancel)?
    } else {
        Sha256::new()
    };

    let client = build_client(cfg)?;
    let host = url.host_str().unwrap_or("").to_string();
    let mut file = OpenOptions::new().create(true).append(true).open(&part)?;
    let speed = Speed {
        started: Instant::now(),
        start_bytes: pos,
    };
    let mut buf = vec![0u8; READ_BLOCK];
    on_progress(&Progress {
        downloaded: pos,
        total: entry.size,
        bytes_per_sec: 0.0,
    });

    while pos < entry.size {
        if cancel.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }
        let end = (pos + cfg.chunk_bytes).min(entry.size) - 1;
        let resp = client
            .get(url.clone())
            .header(RANGE, format!("bytes={pos}-{end}"))
            .send()
            .map_err(|e| classify(&e, &host))?;
        let status = resp.status();
        let mut resp = match status {
            StatusCode::PARTIAL_CONTENT => {
                let header = resp
                    .headers()
                    .get(CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                let (start, total) = parse_content_range(header)
                    .ok_or(DownloadError::BadResponse("нет заголовка Content-Range"))?;
                if start != pos || total != entry.size {
                    return Err(DownloadError::BadResponse(
                        "сервер вернул не тот фрагмент файла",
                    ));
                }
                resp
            }
            // Сервер игнорирует Range и отдаёт файл целиком: начинаем с нуля.
            StatusCode::OK => {
                drop(file);
                file = OpenOptions::new()
                    .create(true)
                    .write(true)
                    .truncate(true)
                    .open(&part)?;
                pos = 0;
                hasher = Sha256::new();
                resp
            }
            s => return Err(status_error(s)),
        };
        let mut remaining_in_chunk = if status == StatusCode::OK {
            u64::MAX
        } else {
            end + 1 - pos
        };
        while remaining_in_chunk > 0 {
            if cancel.is_cancelled() {
                return Err(DownloadError::Cancelled);
            }
            let want = buf
                .len()
                .min(usize::try_from(remaining_in_chunk).unwrap_or(usize::MAX));
            let n = match resp.read(&mut buf[..want]) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::TimedOut => {
                    return Err(DownloadError::Timeout)
                }
                Err(_) => return Err(DownloadError::Interrupted),
            };
            if pos + n as u64 > entry.size {
                return Err(DownloadError::SizeMismatch {
                    got: pos + n as u64,
                    expected: entry.size,
                });
            }
            file.write_all(&buf[..n]).map_err(map_write)?;
            hasher.update(&buf[..n]);
            pos += n as u64;
            remaining_in_chunk = remaining_in_chunk.saturating_sub(n as u64);
            on_progress(&Progress {
                downloaded: pos,
                total: entry.size,
                bytes_per_sec: speed.bytes_per_sec(pos),
            });
        }
        if status != StatusCode::OK && remaining_in_chunk > 0 {
            return Err(DownloadError::Interrupted); // сервер закрыл соединение раньше времени
        }
        if status == StatusCode::OK && pos < entry.size {
            return Err(DownloadError::Interrupted);
        }
    }
    file.flush()?;
    file.sync_all().map_err(map_write)?;
    drop(file);
    finish(entry, &part, &final_path, pos, hasher)
}

/// Проверка размера и суммы и установка файла переименованием.
fn finish(
    entry: &ModelEntry,
    part: &Path,
    final_path: &Path,
    size: u64,
    hasher: Sha256,
) -> Result<PathBuf> {
    if size != entry.size {
        return Err(DownloadError::SizeMismatch {
            got: size,
            expected: entry.size,
        });
    }
    let digest = hex(&hasher.finalize());
    if !digest.eq_ignore_ascii_case(&entry.sha256) {
        let _ = fs::remove_file(part); // повреждённый файл не оставляем: продолжать его бессмысленно
        return Err(DownloadError::HashMismatch);
    }
    fs::rename(part, final_path)?;
    Ok(final_path.to_path_buf())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `bytes 100-199/1000` → (100, 1000).
fn parse_content_range(value: &str) -> Option<(u64, u64)> {
    let rest = value.trim().strip_prefix("bytes ")?;
    let (range, total) = rest.split_once('/')?;
    let (start, _) = range.split_once('-')?;
    Some((start.trim().parse().ok()?, total.trim().parse().ok()?))
}

// ---- импорт из файла -----------------------------------------------------------------------------------

/// Импорт модели из локального файла (флешка, файл от коллеги): без сети. Файл должен совпасть с записью
/// каталога по размеру и SHA-256; неизвестный файл не устанавливается. Исходный файл только читается.
pub fn import_from_file(
    src: &Path,
    models_dir: &Path,
    cancel: &CancelToken,
) -> Result<&'static ModelEntry> {
    import_from_file_with(catalog(), src, models_dir, cancel)
}

pub fn import_from_file_with<'a>(
    entries: &'a [ModelEntry],
    src: &Path,
    models_dir: &Path,
    cancel: &CancelToken,
) -> Result<&'a ModelEntry> {
    let size = fs::metadata(src)?.len();
    let candidates: Vec<&ModelEntry> = entries.iter().filter(|e| e.size == size).collect();
    if candidates.is_empty() {
        return Err(DownloadError::NotACatalogModel);
    }
    fs::create_dir_all(models_dir)?;
    check_space(models_dir, size)?;

    let tmp = models_dir.join(format!(".import-{}.part", std::process::id()));
    let result = copy_and_hash(src, &tmp, cancel).and_then(|digest| {
        let entry = candidates
            .iter()
            .find(|e| digest.eq_ignore_ascii_case(&e.sha256))
            .copied()
            .ok_or(DownloadError::NotACatalogModel)?;
        let dest = model_path(entry, models_dir);
        fs::rename(&tmp, &dest)?;
        Ok(entry)
    });
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn copy_and_hash(src: &Path, dest: &Path, cancel: &CancelToken) -> Result<String> {
    let mut input = File::open(src)?;
    let mut out = File::create(dest)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; READ_BLOCK * 4];
    loop {
        if cancel.is_cancelled() {
            return Err(DownloadError::Cancelled);
        }
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(map_write)?;
        hasher.update(&buf[..n]);
    }
    out.flush()?;
    out.sync_all().map_err(map_write)?;
    Ok(hex(&hasher.finalize()))
}

// ---- проверка соединения -------------------------------------------------------------------------------

/// Проверяет, достижим ли адрес модели с текущими сетевыми настройками (прокси, сертификаты): запрашивает
/// один байт (`Range: bytes=0-0`), ничего не сохраняет. Возвращает итоговый HTTP-статус после редиректов.
/// Нужна для диагностики сети у сотрудников (прокси, блокировка доменов), см. `examples/net_check.rs`.
pub fn check_connection(url: &str, cfg: &DownloadConfig) -> Result<u16> {
    let url = Url::parse(url).map_err(|_| DownloadError::BadResponse("некорректный адрес"))?;
    if !host_allowed(&url, cfg) {
        return Err(DownloadError::UntrustedRedirect(url.to_string()));
    }
    let host = url.host_str().unwrap_or("").to_string();
    let resp = build_client(cfg)?
        .get(url)
        .header(RANGE, "bytes=0-0")
        .send()
        .map_err(|e| classify(&e, &host))?;
    let status = resp.status();
    if status.is_success() {
        Ok(status.as_u16())
    } else {
        Err(status_error(status))
    }
}

/// Что нужно скачать для работы выбранной модели: сама модель и модель определения речи (TD-13),
/// кроме уже установленных. Пустой список — всё на месте. `None` — модели нет в каталоге.
pub fn required_downloads(model_id: &str, models_dir: &Path) -> Option<Vec<&'static ModelEntry>> {
    let main = crate::models::find(model_id)?;
    let vad = crate::models::find(crate::models::VAD_MODEL_ID)?;
    Some(
        [main, vad]
            .into_iter()
            .filter(|e| !is_installed(e, models_dir))
            .collect(),
    )
}
