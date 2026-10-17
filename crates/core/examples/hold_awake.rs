//! Держит запрет сна 20 секунд (для ручной проверки: `pmset -g assertions` на macOS).
fn main() {
    let guard = tolk_core::power::SleepGuard::acquire().expect("запрет сна");
    println!("запрет включён");
    std::thread::sleep(std::time::Duration::from_secs(20));
    drop(guard);
}
