#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(not(windows))]
fn main() {
    eprintln!("ps5cam-setup is available only on Windows.");
}

#[cfg(windows)]
mod locale;

#[cfg(windows)]
mod windows_setup {
    use super::locale::{Text, UiLanguage};
    use std::{
        env,
        ffi::c_void,
        fs::{self, File},
        io::{Cursor, Read, Write},
        os::windows::process::CommandExt,
        path::PathBuf,
        process::Command,
        sync::{Arc, Mutex, OnceLock},
        thread,
    };

    use windows_sys::Win32::{
        Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::Gdi::{
            BeginPaint, CreateCompatibleDC, CreateFontW, CreateSolidBrush, DeleteDC, DeleteObject,
            DrawTextW, Ellipse, EndPaint, FillRect, InvalidateRect, SelectObject, SetBkMode,
            SetTextColor, StretchBlt, UpdateWindow, DT_CENTER, DT_END_ELLIPSIS, DT_LEFT,
            DT_SINGLELINE, DT_VCENTER, DT_WORDBREAK, HBITMAP, HDC, HFONT, HGDIOBJ, PAINTSTRUCT,
            SRCCOPY, TRANSPARENT,
        },
        Graphics::GdiPlus::{
            GdipCreateBitmapFromStream, GdipCreateHBITMAPFromBitmap, GdipCreateHICONFromBitmap,
            GdipDisposeImage, GdipGetImageHeight, GdipGetImageWidth, GdiplusStartup,
            GdiplusStartupInput, GdiplusStartupOutput, GpBitmap, GpImage,
        },
        Storage::FileSystem::{
            GetFileAttributesW, FILE_ATTRIBUTE_DIRECTORY, INVALID_FILE_ATTRIBUTES,
        },
        System::LibraryLoader::GetModuleHandleW,
        System::Services::{
            CloseServiceHandle, OpenSCManagerW, OpenServiceW, SC_MANAGER_CONNECT,
            SERVICE_QUERY_STATUS,
        },
        UI::{
            Shell::{SHCreateMemStream, ShellExecuteW},
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyWindow, DispatchMessageW,
                GetClientRect, GetMessageW, KillTimer, LoadCursorW, PostQuitMessage,
                RegisterClassW, SetTimer, ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
                CW_USEDEFAULT, HICON, IDC_ARROW, MSG, SW_SHOW, WM_CLOSE, WM_DESTROY, WM_LBUTTONUP,
                WM_PAINT, WM_TIMER, WNDCLASSW, WS_CAPTION, WS_MINIMIZEBOX, WS_OVERLAPPED,
                WS_SYSMENU,
            },
        },
    };

    const MAGIC: &[u8; 8] = b"PS5PKG1\0";
    const PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ps5cam-setup.payload"));
    const RELEASE_VERSION: &str = env!("PS5CAM_SETUP_RELEASE_VERSION");
    const HAS_EMBEDDED_PAYLOAD: bool = !cfg!(ps5cam_setup_without_payload);
    const CERTIFICATE_THUMBPRINT: &str = "EDAF55A1E4AE0C8C197988F7286626BD51228CA2";
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const WINDOW_WIDTH: i32 = 880;
    const WINDOW_HEIGHT: i32 = 570;
    const TIMER_ID: usize = 7;
    const TIMER_INTERVAL_MS: u32 = 75;
    const ORIGINAL_PROJECT_URL: &str =
        "https://github.com/raleighlittles/PS5-Camera-Firmware-Loader";
    const CLASS_NAME: &str = "PS5CameraSetupNativeWizard";
    const SERVICE_NAME: &str = "PS5CameraService";
    const PRODUCT_DIRECTORY: &str = "PS5 Camera";
    const PS5_CAMERA_IMAGE: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/ps5-camera-image.jpg"
    ));

    const CANVAS: COLORREF = rgb(246, 248, 251);
    const SURFACE: COLORREF = rgb(255, 255, 255);
    const INK: COLORREF = rgb(28, 43, 61);
    const MUTED: COLORREF = rgb(88, 106, 126);
    const ACCENT: COLORREF = rgb(31, 99, 150);
    const ACCENT_DARK: COLORREF = rgb(22, 72, 112);
    const ACCENT_SOFT: COLORREF = rgb(229, 239, 247);
    const PANEL: COLORREF = rgb(238, 242, 247);
    const SUCCESS: COLORREF = rgb(45, 122, 94);
    const WARNING: COLORREF = rgb(179, 115, 35);

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum OperationOutcome {
        Completed,
        UvcReady,
        BootDetectedUvcTimeout,
        CameraNotConnected,
        RuntimeStatusUnavailable,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum WizardAction {
        Install,
        Uninstall,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum InstallationState {
        NotInstalled,
        Installed,
        NeedsRepair,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Page {
        Welcome,
        Options,
        Review,
        Progress,
        Result,
    }

    #[derive(Debug, Clone)]
    struct WorkerProgress {
        current: u8,
        target: u8,
        stage: String,
        result: Option<Result<OperationOutcome, String>>,
    }

    impl WorkerProgress {
        fn starting(language: UiLanguage) -> Self {
            Self {
                current: 3,
                target: 3,
                stage: language.text(Text::Preparing).to_owned(),
                result: None,
            }
        }
    }

    struct WizardState {
        page: Page,
        action: WizardAction,
        installation: InstallationState,
        remove_certificate: bool,
        language: UiLanguage,
        worker: Option<Arc<Mutex<WorkerProgress>>>,
        result: Option<Result<OperationOutcome, String>>,
        launch_error: Option<String>,
    }

    impl WizardState {
        fn new(
            elevated: bool,
            action: WizardAction,
            remove_certificate: bool,
            installation: InstallationState,
            language: UiLanguage,
        ) -> Self {
            Self {
                page: if elevated {
                    Page::Progress
                } else {
                    Page::Welcome
                },
                action,
                installation,
                remove_certificate,
                language,
                worker: None,
                result: None,
                launch_error: None,
            }
        }
    }

    static WIZARD: OnceLock<Mutex<WizardState>> = OnceLock::new();
    static GDIPLUS_TOKEN: OnceLock<Option<usize>> = OnceLock::new();

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }

    fn text(language: UiLanguage, value: Text) -> &'static str {
        language.text(value)
    }

    const fn rgb(red: u8, green: u8, blue: u8) -> COLORREF {
        red as COLORREF | ((green as COLORREF) << 8) | ((blue as COLORREF) << 16)
    }

    fn ensure_gdiplus() -> bool {
        GDIPLUS_TOKEN
            .get_or_init(|| unsafe {
                let mut token = 0usize;
                let input = GdiplusStartupInput {
                    GdiplusVersion: 1,
                    ..Default::default()
                };
                let mut output = GdiplusStartupOutput::default();
                if GdiplusStartup(&mut token, &input, &mut output) == 0 {
                    Some(token)
                } else {
                    None
                }
            })
            .is_some()
    }

    unsafe fn release_com(object: *mut c_void) {
        if object.is_null() {
            return;
        }
        let vtable = *(object as *mut *mut *mut c_void);
        let release = std::mem::transmute::<
            *mut c_void,
            unsafe extern "system" fn(*mut c_void) -> u32,
        >(*vtable.add(2));
        release(object);
    }

    fn open_ps5_camera_bitmap() -> Option<(*mut c_void, *mut GpBitmap)> {
        if !ensure_gdiplus() {
            return None;
        }
        unsafe {
            let stream =
                SHCreateMemStream(PS5_CAMERA_IMAGE.as_ptr(), PS5_CAMERA_IMAGE.len() as u32);
            if stream.is_null() {
                return None;
            }
            let mut bitmap: *mut GpBitmap = std::ptr::null_mut();
            let loaded = GdipCreateBitmapFromStream(stream, &mut bitmap) == 0 && !bitmap.is_null();
            if !loaded {
                release_com(stream);
                return None;
            }
            Some((stream, bitmap))
        }
    }

    fn paint_ps5_camera(hdc: HDC, x: i32, y: i32, width: i32, height: i32) {
        let Some((stream, bitmap)) = open_ps5_camera_bitmap() else {
            return;
        };
        unsafe {
            let mut source_width = 0;
            let mut source_height = 0;
            let mut rendered_bitmap: HBITMAP = std::ptr::null_mut();
            if GdipGetImageWidth(bitmap as *mut GpImage, &mut source_width) == 0
                && GdipGetImageHeight(bitmap as *mut GpImage, &mut source_height) == 0
                && source_width > 0
                && source_height > 0
                && GdipCreateHBITMAPFromBitmap(bitmap, &mut rendered_bitmap, 0xffff_ffff) == 0
                && !rendered_bitmap.is_null()
            {
                let source = CreateCompatibleDC(hdc);
                if !source.is_null() {
                    let previous = SelectObject(source, rendered_bitmap as HGDIOBJ);
                    StretchBlt(
                        hdc,
                        x,
                        y,
                        width,
                        height,
                        source,
                        0,
                        0,
                        source_width as i32,
                        source_height as i32,
                        SRCCOPY,
                    );
                    SelectObject(source, previous);
                    DeleteDC(source);
                }
                DeleteObject(rendered_bitmap as HGDIOBJ);
            }
            GdipDisposeImage(bitmap as *mut GpImage);
            release_com(stream);
        }
    }

    fn ps5_camera_window_icon() -> HICON {
        let Some((stream, bitmap)) = open_ps5_camera_bitmap() else {
            return std::ptr::null_mut();
        };
        unsafe {
            let mut icon: HICON = std::ptr::null_mut();
            if GdipCreateHICONFromBitmap(bitmap, &mut icon) != 0 {
                icon = std::ptr::null_mut();
            }
            GdipDisposeImage(bitmap as *mut GpImage);
            release_com(stream);
            icon
        }
    }

    fn installation_root_exists() -> bool {
        let program_files = env::var_os("ProgramW6432")
            .or_else(|| env::var_os("ProgramFiles"))
            .unwrap_or_else(|| "C:\\Program Files".into());
        let path = PathBuf::from(program_files).join(PRODUCT_DIRECTORY);
        let path = wide(&path.to_string_lossy());
        unsafe {
            let attributes = GetFileAttributesW(path.as_ptr());
            attributes != INVALID_FILE_ATTRIBUTES && attributes & FILE_ATTRIBUTE_DIRECTORY != 0
        }
    }

    fn service_exists() -> bool {
        let service_name = wide(SERVICE_NAME);
        unsafe {
            let manager = OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT);
            if manager.is_null() {
                return false;
            }
            let service = OpenServiceW(manager, service_name.as_ptr(), SERVICE_QUERY_STATUS);
            let exists = !service.is_null();
            if exists {
                CloseServiceHandle(service);
            }
            CloseServiceHandle(manager);
            exists
        }
    }

    fn classify_installation(root_exists: bool, service_present: bool) -> InstallationState {
        match (root_exists, service_present) {
            (false, false) => InstallationState::NotInstalled,
            (true, true) => InstallationState::Installed,
            _ => InstallationState::NeedsRepair,
        }
    }

    fn detect_installation() -> InstallationState {
        classify_installation(installation_root_exists(), service_exists())
    }

    fn state() -> &'static Mutex<WizardState> {
        WIZARD.get().expect("wizard state must be initialized")
    }

    fn is_in(rect: RECT, x: i32, y: i32) -> bool {
        x >= rect.left && x < rect.right && y >= rect.top && y < rect.bottom
    }

    fn primary_button() -> RECT {
        RECT {
            left: 696,
            top: 482,
            right: 832,
            bottom: 520,
        }
    }

    fn secondary_button() -> RECT {
        RECT {
            left: 550,
            top: 482,
            right: 686,
            bottom: 520,
        }
    }

    fn link_rect() -> RECT {
        RECT {
            left: 50,
            top: 110,
            right: 570,
            bottom: 134,
        }
    }

    fn install_card() -> RECT {
        RECT {
            left: 48,
            top: 242,
            right: 832,
            bottom: 316,
        }
    }

    fn uninstall_card() -> RECT {
        RECT {
            left: 48,
            top: 330,
            right: 832,
            bottom: 404,
        }
    }

    fn certificate_checkbox() -> RECT {
        RECT {
            left: 48,
            top: 390,
            right: 72,
            bottom: 414,
        }
    }

    fn fill(hdc: HDC, rect: RECT, color: COLORREF) {
        unsafe {
            let brush = CreateSolidBrush(color);
            FillRect(hdc, &rect, brush);
            DeleteObject(brush as HGDIOBJ);
        }
    }

    fn paint_text(
        hdc: HDC,
        text: &str,
        mut rect: RECT,
        color: COLORREF,
        size: i32,
        weight: i32,
        flags: u32,
    ) {
        let name = wide("Segoe UI");
        let text = wide(text);
        unsafe {
            let font: HFONT = CreateFontW(
                -size,
                0,
                0,
                0,
                weight,
                0,
                0,
                0,
                1,
                0,
                0,
                0,
                0,
                name.as_ptr(),
            );
            let previous = SelectObject(hdc, font as HGDIOBJ);
            SetBkMode(hdc, TRANSPARENT as i32);
            SetTextColor(hdc, color);
            let flags = if flags & DT_SINGLELINE == 0 {
                flags | DT_WORDBREAK
            } else {
                flags
            };
            DrawTextW(hdc, text.as_ptr(), -1, &mut rect, flags);
            SelectObject(hdc, previous);
            DeleteObject(font as HGDIOBJ);
        }
    }

    fn paint_header(hdc: HDC, page: Page, language: UiLanguage) {
        let step = match page {
            Page::Welcome => text(language, Text::StepStart),
            Page::Options => text(language, Text::StepChoose),
            Page::Review => text(language, Text::StepConfirm),
            Page::Progress => text(language, Text::Applying),
            Page::Result => text(language, Text::Complete),
        };
        paint_text(
            hdc,
            "PS5 Camera Setup",
            RECT {
                left: 48,
                top: 42,
                right: 570,
                bottom: 78,
            },
            INK,
            28,
            700,
            DT_LEFT | DT_SINGLELINE | DT_VCENTER,
        );
        paint_text(
            hdc,
            &format!(
                "{} {RELEASE_VERSION}  ·  {step}",
                text(language, Text::Version)
            ),
            RECT {
                left: 50,
                top: 85,
                right: 570,
                bottom: 108,
            },
            MUTED,
            13,
            400,
            DT_LEFT | DT_SINGLELINE | DT_VCENTER,
        );
        paint_text(
            hdc,
            ORIGINAL_PROJECT_URL,
            link_rect(),
            ACCENT,
            11,
            600,
            DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );
        paint_ps5_camera(hdc, 602, 34, 230, 98);
        fill(
            hdc,
            RECT {
                left: 48,
                top: 158,
                right: 832,
                bottom: 160,
            },
            PANEL,
        );
    }

    fn paint_button(hdc: HDC, rect: RECT, label: &str, primary: bool) {
        fill(hdc, rect, if primary { ACCENT } else { PANEL });
        paint_text(
            hdc,
            label,
            RECT {
                left: rect.left,
                top: rect.top,
                right: rect.right,
                bottom: rect.bottom,
            },
            if primary { SURFACE } else { INK },
            13,
            700,
            DT_CENTER | DT_SINGLELINE | DT_VCENTER,
        );
    }

    fn paint_card(hdc: HDC, rect: RECT, selected: bool, title: &str, description: &str) {
        fill(hdc, rect, if selected { ACCENT_SOFT } else { SURFACE });
        fill(
            hdc,
            RECT {
                left: rect.left,
                top: rect.top,
                right: rect.right,
                bottom: rect.top + 2,
            },
            if selected { ACCENT } else { PANEL },
        );
        unsafe {
            let brush = CreateSolidBrush(if selected { ACCENT } else { rgb(169, 183, 199) });
            let old = SelectObject(hdc, brush as HGDIOBJ);
            Ellipse(
                hdc,
                rect.left + 20,
                rect.top + 24,
                rect.left + 42,
                rect.top + 46,
            );
            SelectObject(hdc, old);
            DeleteObject(brush as HGDIOBJ);
            if selected {
                let dot = CreateSolidBrush(SURFACE);
                let old = SelectObject(hdc, dot as HGDIOBJ);
                Ellipse(
                    hdc,
                    rect.left + 26,
                    rect.top + 30,
                    rect.left + 36,
                    rect.top + 40,
                );
                SelectObject(hdc, old);
                DeleteObject(dot as HGDIOBJ);
            }
        }
        paint_text(
            hdc,
            title,
            RECT {
                left: rect.left + 55,
                top: rect.top + 13,
                right: rect.right - 20,
                bottom: rect.top + 37,
            },
            INK,
            15,
            700,
            DT_LEFT | DT_SINGLELINE | DT_VCENTER,
        );
        paint_text(
            hdc,
            description,
            RECT {
                left: rect.left + 55,
                top: rect.top + 40,
                right: rect.right - 20,
                bottom: rect.bottom - 10,
            },
            MUTED,
            12,
            400,
            DT_LEFT,
        );
    }

    fn paint_welcome(hdc: HDC, installation: InstallationState, language: UiLanguage) {
        let (title, body, notice_title, notice_body, button) = match installation {
            InstallationState::NotInstalled => (
                text(language, Text::NoInstallTitle),
                text(language, Text::NoInstallBody),
                text(language, Text::NoInstallNoticeTitle),
                text(language, Text::NoInstallNoticeBody),
                text(language, Text::InstallGo),
            ),
            InstallationState::Installed => (
                text(language, Text::InstalledTitle),
                text(language, Text::InstalledBody),
                text(language, Text::InstalledNoticeTitle),
                text(language, Text::InstalledNoticeBody),
                text(language, Text::ManageGo),
            ),
            InstallationState::NeedsRepair => (
                text(language, Text::RepairTitle),
                text(language, Text::RepairBody),
                text(language, Text::RepairNoticeTitle),
                text(language, Text::RepairNoticeBody),
                text(language, Text::ReviewOptionsGo),
            ),
        };
        paint_text(
            hdc,
            title,
            RECT {
                left: 48,
                top: 187,
                right: 832,
                bottom: 220,
            },
            INK,
            20,
            700,
            DT_LEFT,
        );
        paint_text(
            hdc,
            body,
            RECT {
                left: 48,
                top: 234,
                right: 832,
                bottom: 301,
            },
            MUTED,
            14,
            400,
            DT_LEFT,
        );
        fill(
            hdc,
            RECT {
                left: 48,
                top: 326,
                right: 832,
                bottom: 416,
            },
            ACCENT_SOFT,
        );
        paint_text(
            hdc,
            notice_title,
            RECT {
                left: 68,
                top: 342,
                right: 812,
                bottom: 365,
            },
            ACCENT_DARK,
            13,
            700,
            DT_LEFT | DT_SINGLELINE,
        );
        paint_text(
            hdc,
            notice_body,
            RECT {
                left: 68,
                top: 371,
                right: 812,
                bottom: 402,
            },
            MUTED,
            12,
            400,
            DT_LEFT,
        );
        paint_button(hdc, primary_button(), button, true);
    }

    fn paint_options(
        hdc: HDC,
        action: WizardAction,
        installation: InstallationState,
        language: UiLanguage,
    ) {
        let reinstall = installation != InstallationState::NotInstalled;
        paint_text(
            hdc,
            if reinstall {
                text(language, Text::ManageExisting)
            } else {
                text(language, Text::ReadyInstall)
            },
            RECT {
                left: 48,
                top: 184,
                right: 832,
                bottom: 214,
            },
            INK,
            20,
            700,
            DT_LEFT,
        );
        paint_text(
            hdc,
            if reinstall {
                text(language, Text::ExistingOptionsBody)
            } else {
                text(language, Text::NewOptionsBody)
            },
            RECT {
                left: 48,
                top: 214,
                right: 832,
                bottom: 234,
            },
            MUTED,
            12,
            400,
            DT_LEFT,
        );
        paint_card(
            hdc,
            install_card(),
            action == WizardAction::Install,
            if reinstall {
                text(language, Text::Reinstall)
            } else {
                text(language, Text::Install)
            },
            if reinstall {
                text(language, Text::ReinstallDescription)
            } else {
                text(language, Text::InstallDescription)
            },
        );
        if reinstall {
            paint_card(
                hdc,
                uninstall_card(),
                action == WizardAction::Uninstall,
                text(language, Text::RemoveFromComputer),
                text(language, Text::RemoveDescription),
            );
        }
        paint_button(hdc, secondary_button(), text(language, Text::Back), false);
        paint_button(hdc, primary_button(), text(language, Text::Review), true);
    }

    fn paint_review(
        hdc: HDC,
        action: WizardAction,
        remove_certificate: bool,
        installation: InstallationState,
        language: UiLanguage,
    ) {
        let (title, body, button) = match action {
            WizardAction::Install => (
                if installation == InstallationState::NotInstalled {
                    text(language, Text::ReadyInstall)
                } else {
                    text(language, Text::ReadyReinstall)
                },
                text(language, Text::ReviewInstallBody),
                if installation == InstallationState::NotInstalled {
                    text(language, Text::InstallNow)
                } else {
                    text(language, Text::ReinstallNow)
                },
            ),
            WizardAction::Uninstall => (
                text(language, Text::ReadyRemove),
                text(language, Text::ReviewRemoveBody),
                text(language, Text::RemoveNow),
            ),
        };
        paint_text(
            hdc,
            title,
            RECT {
                left: 48,
                top: 184,
                right: 832,
                bottom: 214,
            },
            INK,
            20,
            700,
            DT_LEFT,
        );
        paint_text(
            hdc,
            body,
            RECT {
                left: 48,
                top: 230,
                right: 832,
                bottom: 316,
            },
            MUTED,
            13,
            400,
            DT_LEFT,
        );
        if action == WizardAction::Uninstall {
            let checkbox = certificate_checkbox();
            fill(
                hdc,
                checkbox,
                if remove_certificate { ACCENT } else { PANEL },
            );
            if remove_certificate {
                paint_text(
                    hdc,
                    "✓",
                    RECT {
                        left: checkbox.left + 7,
                        top: checkbox.top + 1,
                        right: checkbox.right,
                        bottom: checkbox.bottom,
                    },
                    SURFACE,
                    14,
                    700,
                    DT_LEFT,
                );
            }
            paint_text(
                hdc,
                text(language, Text::RemoveCertificate),
                RECT {
                    left: 84,
                    top: 390,
                    right: 832,
                    bottom: 414,
                },
                INK,
                12,
                600,
                DT_LEFT,
            );
            paint_text(
                hdc,
                text(language, Text::RemoveCertificateDetail),
                RECT {
                    left: 84,
                    top: 414,
                    right: 832,
                    bottom: 440,
                },
                MUTED,
                11,
                400,
                DT_LEFT,
            );
        } else {
            fill(
                hdc,
                RECT {
                    left: 48,
                    top: 358,
                    right: 832,
                    bottom: 424,
                },
                ACCENT_SOFT,
            );
            paint_text(
                hdc,
                text(language, Text::MemoryNotice),
                RECT {
                    left: 68,
                    top: 376,
                    right: 812,
                    bottom: 409,
                },
                ACCENT_DARK,
                12,
                600,
                DT_LEFT,
            );
        }
        paint_button(hdc, secondary_button(), text(language, Text::Back), false);
        paint_button(hdc, primary_button(), button, true);
    }

    fn paint_progress(
        hdc: HDC,
        action: WizardAction,
        progress: &WorkerProgress,
        installation: InstallationState,
        language: UiLanguage,
    ) {
        paint_text(
            hdc,
            if action == WizardAction::Install {
                if installation == InstallationState::NotInstalled {
                    text(language, Text::Installing)
                } else {
                    text(language, Text::Reinstalling)
                }
            } else {
                text(language, Text::Removing)
            },
            RECT {
                left: 48,
                top: 184,
                right: 832,
                bottom: 214,
            },
            INK,
            20,
            700,
            DT_LEFT,
        );
        paint_text(
            hdc,
            text(language, Text::WindowStaysOpen),
            RECT {
                left: 48,
                top: 224,
                right: 832,
                bottom: 248,
            },
            MUTED,
            13,
            400,
            DT_LEFT,
        );
        fill(
            hdc,
            RECT {
                left: 48,
                top: 278,
                right: 832,
                bottom: 296,
            },
            PANEL,
        );
        if progress.current > 0 {
            fill(
                hdc,
                RECT {
                    left: 48,
                    top: 278,
                    right: 48 + 784 * i32::from(progress.current) / 100,
                    bottom: 296,
                },
                ACCENT,
            );
        }
        paint_text(
            hdc,
            &format!("{}%", progress.current),
            RECT {
                left: 48,
                top: 308,
                right: 122,
                bottom: 333,
            },
            ACCENT,
            13,
            700,
            DT_LEFT,
        );
        paint_text(
            hdc,
            &progress.stage,
            RECT {
                left: 48,
                top: 358,
                right: 832,
                bottom: 409,
            },
            MUTED,
            13,
            400,
            DT_LEFT,
        );
        paint_text(
            hdc,
            text(language, Text::DoNotDisconnect),
            RECT {
                left: 48,
                top: 434,
                right: 832,
                bottom: 456,
            },
            MUTED,
            11,
            400,
            DT_LEFT,
        );
    }

    fn result_text(
        result: &Result<OperationOutcome, String>,
        action: WizardAction,
        language: UiLanguage,
    ) -> (String, String, bool) {
        match result {
            Ok(OperationOutcome::Completed) => (
                text(language, Text::RemovedTitle).to_owned(),
                text(language, Text::RemovedBody).to_owned(),
                true,
            ),
            Ok(OperationOutcome::UvcReady) => (
                text(language, Text::CameraReadyTitle).to_owned(),
                text(language, Text::CameraReadyBody).to_owned(),
                true,
            ),
            Ok(OperationOutcome::BootDetectedUvcTimeout) => (
                text(language, Text::CameraPendingTitle).to_owned(),
                text(language, Text::CameraPendingBody).to_owned(),
                false,
            ),
            Ok(OperationOutcome::CameraNotConnected) => (
                text(language, Text::InstallationPreparedTitle).to_owned(),
                text(language, Text::InstallationPreparedBody).to_owned(),
                true,
            ),
            Ok(OperationOutcome::RuntimeStatusUnavailable) => (
                text(language, Text::VerificationPendingTitle).to_owned(),
                text(language, Text::VerificationPendingBody).to_owned(),
                false,
            ),
            Err(error) => (
                if action == WizardAction::Install {
                    text(language, Text::InstallationFailed)
                } else {
                    text(language, Text::RemovalFailed)
                }
                .to_owned(),
                error.clone(),
                false,
            ),
        }
    }

    fn paint_result(
        hdc: HDC,
        result: &Result<OperationOutcome, String>,
        action: WizardAction,
        language: UiLanguage,
    ) {
        let (title, body, success) = result_text(result, action, language);
        unsafe {
            let brush = CreateSolidBrush(if success { SUCCESS } else { WARNING });
            let old = SelectObject(hdc, brush as HGDIOBJ);
            Ellipse(hdc, 48, 185, 103, 240);
            SelectObject(hdc, old);
            DeleteObject(brush as HGDIOBJ);
        }
        paint_text(
            hdc,
            if success { "✓" } else { "!" },
            RECT {
                left: 66,
                top: 192,
                right: 92,
                bottom: 233,
            },
            SURFACE,
            25,
            700,
            DT_LEFT,
        );
        paint_text(
            hdc,
            &title,
            RECT {
                left: 122,
                top: 187,
                right: 832,
                bottom: 220,
            },
            INK,
            20,
            700,
            DT_LEFT,
        );
        paint_text(
            hdc,
            &body,
            RECT {
                left: 48,
                top: 278,
                right: 832,
                bottom: 375,
            },
            MUTED,
            13,
            400,
            DT_LEFT,
        );
        paint_button(hdc, primary_button(), text(language, Text::Close), true);
    }

    fn paint(hwnd: HWND) {
        unsafe {
            let mut paint = std::mem::zeroed::<PAINTSTRUCT>();
            let hdc = BeginPaint(hwnd, &mut paint);
            let mut client = std::mem::zeroed::<RECT>();
            GetClientRect(hwnd, &mut client);
            fill(hdc, client, CANVAS);
            let locked = state().lock().expect("wizard state poisoned");
            paint_header(hdc, locked.page, locked.language);
            match locked.page {
                Page::Welcome => paint_welcome(hdc, locked.installation, locked.language),
                Page::Options => {
                    paint_options(hdc, locked.action, locked.installation, locked.language)
                }
                Page::Review => paint_review(
                    hdc,
                    locked.action,
                    locked.remove_certificate,
                    locked.installation,
                    locked.language,
                ),
                Page::Progress => {
                    let progress = locked
                        .worker
                        .as_ref()
                        .and_then(|worker| worker.lock().ok().map(|value| value.clone()))
                        .unwrap_or_else(|| WorkerProgress::starting(locked.language));
                    paint_progress(
                        hdc,
                        locked.action,
                        &progress,
                        locked.installation,
                        locked.language,
                    );
                }
                Page::Result => {
                    let result = locked.result.clone().unwrap_or_else(|| {
                        Err(locked.launch_error.clone().unwrap_or_else(|| {
                            text(locked.language, Text::NoUsableResult).to_owned()
                        }))
                    });
                    paint_result(hdc, &result, locked.action, locked.language);
                }
            }
            EndPaint(hwnd, &paint);
        }
    }

    fn set_progress(progress: &Arc<Mutex<WorkerProgress>>, target: u8, stage: impl Into<String>) {
        let mut value = progress.lock().expect("progress state poisoned");
        value.target = target;
        value.stage = stage.into();
    }

    fn start_engine(
        action: WizardAction,
        remove_certificate: bool,
        language: UiLanguage,
    ) -> Arc<Mutex<WorkerProgress>> {
        let progress = Arc::new(Mutex::new(WorkerProgress::starting(language)));
        let worker_progress = Arc::clone(&progress);
        thread::spawn(move || {
            let requested = match action {
                WizardAction::Install => "Repair",
                WizardAction::Uninstall => "Uninstall",
            };
            let result = run_engine(requested, remove_certificate, &worker_progress, language);
            let mut value = worker_progress.lock().expect("progress state poisoned");
            value.target = 100;
            value.stage = match &result {
                Ok(_) => text(language, Text::Finalizing).to_owned(),
                Err(_) => text(language, Text::ReviewMessage).to_owned(),
            };
            value.result = Some(result);
        });
        progress
    }

    fn elevate(
        action: WizardAction,
        remove_certificate: bool,
        language: UiLanguage,
    ) -> Result<(), String> {
        let exe = env::current_exe().map_err(|error| error.to_string())?;
        let operation = wide("runas");
        let file = wide(&exe.to_string_lossy());
        let mut parameters = format!(
            "--elevated --wizard-action {}",
            match action {
                WizardAction::Install => "install",
                WizardAction::Uninstall => "uninstall",
            }
        );
        if remove_certificate {
            parameters.push_str(" --remove-development-certificate");
        }
        let parameters = wide(&parameters);
        let result = unsafe {
            ShellExecuteW(
                std::ptr::null_mut(),
                operation.as_ptr(),
                file.as_ptr(),
                parameters.as_ptr(),
                std::ptr::null(),
                SW_SHOW,
            )
        };
        if result as isize <= 32 {
            return Err(text(language, Text::ElevationFailed)
                .replace("{code}", &(result as isize).to_string()));
        }
        Ok(())
    }

    fn handle_click(hwnd: HWND, x: i32, y: i32) {
        let mut close_after_elevation = false;
        {
            let mut locked = state().lock().expect("wizard state poisoned");
            if is_in(link_rect(), x, y) {
                let operation = wide("open");
                let url = wide(ORIGINAL_PROJECT_URL);
                unsafe {
                    ShellExecuteW(
                        hwnd,
                        operation.as_ptr(),
                        url.as_ptr(),
                        std::ptr::null(),
                        std::ptr::null(),
                        SW_SHOW,
                    );
                }
                return;
            }
            match locked.page {
                Page::Welcome if is_in(primary_button(), x, y) => {
                    locked.page = if locked.installation == InstallationState::NotInstalled {
                        Page::Review
                    } else {
                        Page::Options
                    };
                }
                Page::Options => {
                    if is_in(install_card(), x, y) {
                        locked.action = WizardAction::Install;
                    } else if locked.installation != InstallationState::NotInstalled
                        && is_in(uninstall_card(), x, y)
                    {
                        locked.action = WizardAction::Uninstall;
                    } else if is_in(secondary_button(), x, y) {
                        locked.page = Page::Welcome;
                    } else if is_in(primary_button(), x, y) {
                        locked.page = Page::Review;
                    }
                }
                Page::Review => {
                    if locked.action == WizardAction::Uninstall
                        && is_in(certificate_checkbox(), x, y)
                    {
                        locked.remove_certificate = !locked.remove_certificate;
                    } else if is_in(secondary_button(), x, y) {
                        locked.page = if locked.installation == InstallationState::NotInstalled {
                            Page::Welcome
                        } else {
                            Page::Options
                        };
                    } else if is_in(primary_button(), x, y) {
                        match elevate(locked.action, locked.remove_certificate, locked.language) {
                            Ok(()) => close_after_elevation = true,
                            Err(error) => {
                                locked.launch_error = Some(error);
                                locked.result = None;
                                locked.page = Page::Result;
                            }
                        }
                    }
                }
                Page::Result if is_in(primary_button(), x, y) => unsafe {
                    DestroyWindow(hwnd);
                },
                Page::Progress | Page::Result | Page::Welcome => {}
            }
        }
        if close_after_elevation {
            unsafe { DestroyWindow(hwnd) };
        } else {
            unsafe { InvalidateRect(hwnd, std::ptr::null(), 1) };
        }
    }

    fn update_progress(hwnd: HWND) {
        let mut locked = state().lock().expect("wizard state poisoned");
        if locked.page != Page::Progress {
            return;
        }
        let Some(worker) = locked.worker.clone() else {
            return;
        };
        let completed = {
            let mut worker = worker.lock().expect("progress state poisoned");
            if worker.current < worker.target {
                worker.current = worker.current.saturating_add(2).min(worker.target);
            }
            if worker.current == 100 {
                worker.result.take()
            } else {
                None
            }
        };
        if let Some(result) = completed {
            locked.result = Some(result);
            locked.page = Page::Result;
            unsafe { KillTimer(hwnd, TIMER_ID) };
        }
        drop(locked);
        unsafe { InvalidateRect(hwnd, std::ptr::null(), 1) };
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        w_param: WPARAM,
        l_param: LPARAM,
    ) -> LRESULT {
        match message {
            WM_PAINT => {
                paint(hwnd);
                0
            }
            WM_LBUTTONUP => {
                let x = (l_param as u32 & 0xffff) as i16 as i32;
                let y = ((l_param as u32 >> 16) & 0xffff) as i16 as i32;
                handle_click(hwnd, x, y);
                0
            }
            WM_TIMER if w_param == TIMER_ID => {
                update_progress(hwnd);
                0
            }
            WM_CLOSE => {
                let working = state()
                    .lock()
                    .map(|locked| locked.page == Page::Progress)
                    .unwrap_or(false);
                if !working {
                    DestroyWindow(hwnd);
                }
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, message, w_param, l_param),
        }
    }

    fn run_wizard(
        elevated: bool,
        action: WizardAction,
        remove_certificate: bool,
    ) -> Result<(), String> {
        let installation = detect_installation();
        let language = UiLanguage::system();
        WIZARD
            .set(Mutex::new(WizardState::new(
                elevated,
                action,
                remove_certificate,
                installation,
                language,
            )))
            .map_err(|_| "O instalador já possui uma janela ativa.".to_owned())?;
        if elevated {
            let mut locked = state().lock().expect("wizard state poisoned");
            locked.worker = Some(start_engine(
                locked.action,
                locked.remove_certificate,
                locked.language,
            ));
        }
        let class_name = wide(CLASS_NAME);
        let title = wide(&format!("PS5 Camera Setup V{RELEASE_VERSION}"));
        let icon = ps5_camera_window_icon();
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let mut class = std::mem::zeroed::<WNDCLASSW>();
            class.style = CS_HREDRAW | CS_VREDRAW;
            class.lpfnWndProc = Some(window_proc);
            class.hInstance = instance;
            class.hIcon = icon;
            class.hCursor = LoadCursorW(std::ptr::null_mut(), IDC_ARROW);
            class.lpszClassName = class_name.as_ptr();
            RegisterClassW(&class);
            let hwnd = CreateWindowExW(
                0,
                class_name.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                WINDOW_WIDTH + 24,
                WINDOW_HEIGHT + 58,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null::<c_void>(),
            );
            if hwnd.is_null() {
                if !icon.is_null() {
                    DestroyIcon(icon);
                }
                return Err("Não foi possível abrir a janela do instalador.".to_owned());
            }
            ShowWindow(hwnd, SW_SHOW);
            UpdateWindow(hwnd);
            if elevated {
                SetTimer(hwnd, TIMER_ID, TIMER_INTERVAL_MS, None);
            }
            let mut message = std::mem::zeroed::<MSG>();
            while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            if !icon.is_null() {
                DestroyIcon(icon);
            }
        }
        Ok(())
    }

    fn read_u16(cursor: &mut Cursor<&[u8]>) -> Result<u16, String> {
        let mut bytes = [0; 2];
        cursor
            .read_exact(&mut bytes)
            .map_err(|error| error.to_string())?;
        Ok(u16::from_le_bytes(bytes))
    }

    fn read_u64(cursor: &mut Cursor<&[u8]>) -> Result<u64, String> {
        let mut bytes = [0; 8];
        cursor
            .read_exact(&mut bytes)
            .map_err(|error| error.to_string())?;
        Ok(u64::from_le_bytes(bytes))
    }

    fn extract_payload() -> Result<PathBuf, String> {
        if !HAS_EMBEDDED_PAYLOAD {
            return Err(UiLanguage::system()
                .text(Text::PayloadUnavailable)
                .to_owned());
        }
        let mut cursor = Cursor::new(PAYLOAD);
        let mut magic = [0; 8];
        cursor
            .read_exact(&mut magic)
            .map_err(|error| error.to_string())?;
        if &magic != MAGIC {
            return Err("O payload interno do instalador está corrompido.".into());
        }
        let destination = env::temp_dir().join(format!("PS5CameraSetup-{}", std::process::id()));
        fs::create_dir_all(&destination).map_err(|error| error.to_string())?;
        for _ in 0..read_u16(&mut cursor)? {
            let name_length = read_u16(&mut cursor)? as usize;
            let mut name = vec![0; name_length];
            cursor
                .read_exact(&mut name)
                .map_err(|error| error.to_string())?;
            let name = String::from_utf8(name)
                .map_err(|_| "Nome inválido no payload interno.".to_owned())?;
            if name.is_empty() || name.contains(['\\', '/', ':']) {
                return Err("O payload interno contém um caminho inseguro.".into());
            }
            let length = read_u64(&mut cursor)? as usize;
            if length > 128 * 1024 * 1024 {
                return Err("O payload interno contém um arquivo grande demais.".into());
            }
            let mut bytes = vec![0; length];
            cursor
                .read_exact(&mut bytes)
                .map_err(|error| error.to_string())?;
            File::create(destination.join(name))
                .map_err(|error| error.to_string())?
                .write_all(&bytes)
                .map_err(|error| error.to_string())?;
        }
        Ok(destination)
    }

    fn run_engine(
        action: &str,
        remove_certificate: bool,
        progress: &Arc<Mutex<WorkerProgress>>,
        language: UiLanguage,
    ) -> Result<OperationOutcome, String> {
        set_progress(progress, 12, text(language, Text::Extracting));
        let payload = extract_payload()?;
        let engine = payload.join("PS5CameraDevelopmentInstaller.ps1");
        let manifest = payload.join("release-manifest.json");
        let power_shell = env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\Windows"))
            .join("System32\\WindowsPowerShell\\v1.0\\powershell.exe");
        let mut command = Command::new(power_shell);
        command.creation_flags(CREATE_NO_WINDOW);
        command
            .args([
                "-NoLogo",
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(&engine)
            .args([
                "-Action",
                action,
                "-ConfirmDevelopmentCertificateThumbprint",
                CERTIFICATE_THUMBPRINT,
                "-EmbeddedPayload",
                "-Execute",
            ]);
        if action != "Uninstall" {
            command
                .args(["-ReleaseManifest"])
                .arg(&manifest)
                .args(["-ConfirmReleaseVersion", RELEASE_VERSION]);
        }
        if remove_certificate {
            command.arg("-RemoveDevelopmentCertificate");
        }
        set_progress(progress, 40, text(language, Text::ApplyingChange));
        let output = command.output().map_err(|error| {
            text(language, Text::EngineStartFailed).replace("{error}", &error.to_string())
        })?;
        set_progress(progress, 90, text(language, Text::CheckingResult));
        let _ = fs::remove_dir_all(&payload);
        if output.status.success() {
            if action == "Uninstall" {
                return Ok(OperationOutcome::Completed);
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("\"device_status\":\"uvc_ready\"") {
                Ok(OperationOutcome::UvcReady)
            } else if stdout.contains("\"device_status\":\"boot_detected_uvc_timeout\"") {
                Ok(OperationOutcome::BootDetectedUvcTimeout)
            } else if stdout.contains("\"device_status\":\"camera_not_connected\"") {
                Ok(OperationOutcome::CameraNotConnected)
            } else {
                Ok(OperationOutcome::RuntimeStatusUnavailable)
            }
        } else {
            Err(command_failure(&output, language))
        }
    }

    fn command_failure(output: &std::process::Output, language: UiLanguage) -> String {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        if !stderr.is_empty() {
            return stderr;
        }
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !stdout.is_empty() {
            return stdout;
        }
        text(language, Text::EngineEnded).replace("{status}", &output.status.to_string())
    }

    fn verify_embedded_payload() -> Result<(), String> {
        let payload = extract_payload()?;
        let engine = payload.join("PS5CameraDevelopmentInstaller.ps1");
        let manifest = payload.join("release-manifest.json");
        let power_shell = env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("C:\\Windows"))
            .join("System32\\WindowsPowerShell\\v1.0\\powershell.exe");
        let output = Command::new(power_shell)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(engine)
            .args(["-Action", "Repair", "-ReleaseManifest"])
            .arg(manifest)
            .args([
                "-ConfirmReleaseVersion",
                RELEASE_VERSION,
                "-ConfirmDevelopmentCertificateThumbprint",
                CERTIFICATE_THUMBPRINT,
                "-EmbeddedPayload",
            ])
            .output()
            .map_err(|error| error.to_string())?;
        let _ = fs::remove_dir_all(&payload);
        if output.status.success() {
            Ok(())
        } else {
            Err(command_failure(&output, UiLanguage::system()))
        }
    }

    fn requested_action(arguments: &[String]) -> WizardAction {
        match arguments
            .windows(2)
            .find(|pair| pair[0] == "--wizard-action" || pair[0] == "--action")
            .map(|pair| pair[1].as_str())
        {
            Some("uninstall") | Some("Uninstall") => WizardAction::Uninstall,
            _ => WizardAction::Install,
        }
    }

    pub fn main() {
        let arguments: Vec<String> = env::args().collect();
        if arguments
            .iter()
            .any(|argument| argument == "--verify-payload")
        {
            match verify_embedded_payload() {
                Ok(()) => println!("embedded payload verified: {RELEASE_VERSION}"),
                Err(error) => {
                    eprintln!("embedded payload verification failed: {error}");
                    std::process::exit(1);
                }
            }
            return;
        }
        let elevated = arguments.iter().any(|argument| argument == "--elevated");
        let action = requested_action(&arguments);
        let remove_certificate = arguments
            .iter()
            .any(|argument| argument == "--remove-development-certificate");
        if let Err(error) = run_wizard(elevated, action, remove_certificate) {
            eprintln!("PS5 Camera Setup failed: {error}");
            std::process::exit(1);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn unpackaged_build_refuses_payload_extraction() {
            if !HAS_EMBEDDED_PAYLOAD {
                assert!(extract_payload().is_err());
            }
        }

        #[test]
        fn opens_the_embedded_ps5_camera_image_with_gdiplus() {
            assert_eq!(&PS5_CAMERA_IMAGE[..3], b"\xff\xd8\xff");
            let (stream, bitmap) = open_ps5_camera_bitmap().expect("decodable PS5 camera image");
            let mut width = 0;
            let mut height = 0;
            unsafe {
                assert_eq!(GdipGetImageWidth(bitmap as *mut GpImage, &mut width), 0);
                assert_eq!(GdipGetImageHeight(bitmap as *mut GpImage, &mut height), 0);
                GdipDisposeImage(bitmap as *mut GpImage);
                release_com(stream);
            }
            assert!(width > height);
            assert!(height > 100);
        }

        #[test]
        fn creates_a_window_icon_from_the_embedded_ps5_camera_image() {
            let icon = ps5_camera_window_icon();
            assert!(!icon.is_null());
            unsafe {
                DestroyIcon(icon);
            }
        }

        #[test]
        fn classifies_installation_only_when_service_and_files_agree() {
            assert_eq!(
                classify_installation(false, false),
                InstallationState::NotInstalled
            );
            assert_eq!(
                classify_installation(true, true),
                InstallationState::Installed
            );
            assert_eq!(
                classify_installation(true, false),
                InstallationState::NeedsRepair
            );
            assert_eq!(
                classify_installation(false, true),
                InstallationState::NeedsRepair
            );
        }
    }
}

#[cfg(windows)]
fn main() {
    windows_setup::main();
}
