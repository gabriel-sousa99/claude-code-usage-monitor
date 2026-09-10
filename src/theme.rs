use windows::core::PCWSTR;
use windows::Win32::System::Registry::*;

use crate::native_interop::wide_str;

const REGISTRY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
const REGISTRY_KEY: &str = "SystemUsesLightTheme";

const ACCENT_REGISTRY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent";
const ACCENT_REGISTRY_KEY: &str = "AccentPalette";

/// A paleta traz oito tons de 4 bytes, do mais claro para o mais escuro:
/// Light3, Light2, Light1, Accent, Dark1, Dark2, Dark3 e um extra.
const ACCENT_PALETTE_LEN: usize = 32;
const ACCENT_LIGHT2_OFFSET: usize = 4;
const ACCENT_DARK1_OFFSET: usize = 16;

/// Tom de acento que o Windows aplica aos elementos em destaque: um tom claro
/// sobre fundo escuro e um escuro sobre fundo claro, para o contraste não cair.
/// Sem a paleta no registro, devolve o lilás padrão do Windows 11.
pub fn accent_color() -> (u8, u8, u8) {
    let offset = if is_dark_mode() {
        ACCENT_LIGHT2_OFFSET
    } else {
        ACCENT_DARK1_OFFSET
    };

    match accent_palette() {
        Some(palette) => (palette[offset], palette[offset + 1], palette[offset + 2]),
        None => (0xDB, 0x9E, 0xE5),
    }
}

fn accent_palette() -> Option<[u8; ACCENT_PALETTE_LEN]> {
    unsafe {
        let path = wide_str(ACCENT_REGISTRY_PATH);
        let key_name = wide_str(ACCENT_REGISTRY_KEY);

        let mut hkey = HKEY::default();
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(path.as_ptr()),
            None,
            KEY_READ,
            &mut hkey,
        )
        .is_err()
        {
            return None;
        }

        let mut data = [0u8; ACCENT_PALETTE_LEN];
        let mut data_size = ACCENT_PALETTE_LEN as u32;
        let result = RegQueryValueExW(
            hkey,
            PCWSTR::from_raw(key_name.as_ptr()),
            None,
            None,
            Some(data.as_mut_ptr()),
            Some(&mut data_size),
        );

        let _ = RegCloseKey(hkey);

        (result.is_ok() && data_size as usize == ACCENT_PALETTE_LEN).then_some(data)
    }
}

/// Check if the system is in dark mode by reading the registry
pub fn is_dark_mode() -> bool {
    !is_light_theme()
}

fn is_light_theme() -> bool {
    unsafe {
        let path = wide_str(REGISTRY_PATH);
        let key_name = wide_str(REGISTRY_KEY);

        let mut hkey = HKEY::default();
        let result = RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR::from_raw(path.as_ptr()),
            None,
            KEY_READ,
            &mut hkey,
        );

        if result.is_err() {
            return false; // Default to dark mode
        }

        let mut data: u32 = 0;
        let mut data_size: u32 = std::mem::size_of::<u32>() as u32;
        let result = RegQueryValueExW(
            hkey,
            PCWSTR::from_raw(key_name.as_ptr()),
            None,
            None,
            Some(&mut data as *mut u32 as *mut u8),
            Some(&mut data_size),
        );

        let _ = RegCloseKey(hkey);

        if result.is_err() {
            return false; // Default to dark mode
        }

        data == 1
    }
}
