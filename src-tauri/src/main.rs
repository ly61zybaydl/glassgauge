#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod accounts;
mod discovery;
mod engine;
mod relay;
mod token;
mod wallpaper;
mod window;

fn main() {
    tauri::Builder::default()
        .manage(relay::RelayState::default())
        .manage(accounts::AccountsLock::default())
        .invoke_handler(tauri::generate_handler![
            window::get_config,
            window::get_glass_mode,
            window::save_state,
            window::set_glass,
            relay::fetch_limits,
            wallpaper::get_wallpaper,
            accounts::accounts_list,
            accounts::accounts_save,
            accounts::accounts_switch,
            accounts::accounts_remove,
        ])
        .setup(|app| {
            window::setup(app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running glassgauge");
}
