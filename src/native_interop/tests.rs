use super::*;
use std::cell::RefCell;
use windows::Win32::Foundation::LRESULT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;

fn monitor(index: isize, rect: RECT, primary: bool) -> DisplayMonitor {
    DisplayMonitor {
        handle: HMONITOR(index as *mut _),
        rect,
        primary,
    }
}

fn taskbar(index: isize, rect: RECT, primary: bool) -> TaskbarWindow {
    TaskbarWindow {
        hwnd: HWND(index as *mut _),
        rect,
        primary,
    }
}

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RECT {
    RECT {
        left,
        top,
        right,
        bottom,
    }
}

#[test]
fn offscreen_secondary_never_displaces_the_main_taskbar() {
    let displays = [monitor(1, rect(0, 0, 1920, 1080), true)];
    let bars = [
        taskbar(2, rect(-1920, -100, 0, -52), false),
        taskbar(1, rect(0, 1032, 1920, 1080), true),
    ];
    for bars in [bars, [bars[1], bars[0]]] {
        assert_eq!(
            taskbar_for_monitor(&bars, &displays, displays[0].handle)
                .unwrap()
                .hwnd,
            bars.iter().find(|bar| bar.primary).unwrap().hwnd
        );
        assert_eq!(
            monitor_for_taskbar(&bars, &displays, HWND(2 as *mut _)),
            None
        );
    }
}

#[test]
fn secondary_taskbars_use_greatest_overlap_and_visible_candidates_win() {
    let displays = [
        monitor(1, rect(0, 0, 1920, 1080), true),
        monitor(2, rect(-1920, -200, 0, 880), false),
    ];
    let bars = [
        taskbar(3, rect(-1920, -248, 0, -200), false),
        taskbar(2, rect(-1920, 832, 20, 880), false),
        taskbar(1, rect(0, 1032, 1920, 1080), true),
    ];
    assert_eq!(
        taskbar_for_monitor(&bars, &displays, displays[1].handle)
            .unwrap()
            .hwnd,
        bars[1].hwnd
    );
    assert_eq!(
        monitor_for_taskbar(&bars, &displays, bars[1].hwnd),
        Some(displays[1].handle)
    );
    assert_eq!(monitor_for_taskbar(&bars, &displays, bars[0].hwnd), None);
}

#[test]
fn offscreen_autohide_bar_is_a_fallback_for_its_nearest_display() {
    let displays = [
        monitor(1, rect(0, 0, 1920, 1080), true),
        monitor(2, rect(-1920, 0, 0, 1080), false),
    ];
    let bars = [
        taskbar(1, rect(0, 1078, 1920, 1126), true),
        taskbar(2, rect(-1920, 1080, 0, 1128), false),
    ];
    assert_eq!(
        taskbar_for_monitor(&bars, &displays, displays[1].handle)
            .unwrap()
            .hwnd,
        bars[1].hwnd
    );
    assert_eq!(
        taskbar_for_monitor(&bars, &displays[..1], displays[0].handle)
            .unwrap()
            .hwnd,
        bars[0].hwnd
    );
    assert!(taskbar_for_monitor(&bars, &[], displays[0].handle).is_none());
}

#[test]
fn browser_opener_rejects_non_web_targets_without_launching_them() {
    for target in [
        "",
        "cmd.exe",
        "file:///C:/Windows/System32/cmd.exe",
        "ms-settings:",
        "https://example.com\0cmd.exe",
    ] {
        assert!(!open_web_url(None, target));
    }
}

#[derive(Debug)]
struct TransitionObservation {
    message: u32,
    visible: bool,
    popup: bool,
    rect: (i32, i32, i32, i32),
}

thread_local! {
    static OBSERVATIONS: RefCell<Vec<TransitionObservation>> = const { RefCell::new(Vec::new()) };
}

unsafe extern "system" fn observe_transition(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let result = DefWindowProcW(hwnd, message, wparam, lparam);
    if matches!(
        message,
        WM_STYLECHANGED | WM_WINDOWPOSCHANGED | WM_SHOWWINDOW
    ) {
        if let Some(rect) = get_window_rect_safe(hwnd) {
            OBSERVATIONS.with(|observations| {
                observations.borrow_mut().push(TransitionObservation {
                    message,
                    visible: IsWindowVisible(hwnd).as_bool(),
                    popup: GetWindowLongW(hwnd, GWL_STYLE) as u32 & WS_CHILD_STYLE == 0,
                    rect: (rect.left, rect.top, rect.right, rect.bottom),
                });
            });
        }
    }
    result
}

struct TestWindow(HWND);

impl Drop for TestWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}

#[test]
fn detaching_a_child_never_exposes_parent_relative_coordinates_as_a_popup() {
    unsafe {
        let class = wide_str("UsageMonitorPopupTransitionRegression");
        let instance = GetModuleHandleW(PCWSTR::null()).unwrap();
        let atom = RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(observe_transition),
            hInstance: instance.into(),
            lpszClassName: PCWSTR(class.as_ptr()),
            ..Default::default()
        });
        assert_ne!(atom, 0);

        // Transparent, non-activating private windows exercise the real Win32
        // transition without touching Explorer or displaying a test window.
        let parent = TestWindow(
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                PCWSTR(class.as_ptr()),
                PCWSTR::null(),
                WS_POPUP,
                600,
                900,
                600,
                50,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .unwrap(),
        );
        SetLayeredWindowAttributes(parent.0, Default::default(), 0, LWA_ALPHA).unwrap();
        let _ = ShowWindow(parent.0, SW_SHOWNOACTIVATE);

        for initially_visible in [true, false] {
            let child = TestWindow(
                CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    PCWSTR(class.as_ptr()),
                    PCWSTR::null(),
                    WS_CHILD,
                    180,
                    2,
                    217,
                    46,
                    Some(parent.0),
                    None,
                    Some(instance.into()),
                    None,
                )
                .unwrap(),
            );
            SetLayeredWindowAttributes(child.0, Default::default(), 0, LWA_ALPHA).unwrap();
            if initially_visible {
                let _ = ShowWindow(child.0, SW_SHOWNOACTIVATE);
            }
            let before = get_window_rect_safe(child.0).unwrap();
            let expected = (before.left, before.top, before.right, before.bottom);
            OBSERVATIONS.with(|observations| observations.borrow_mut().clear());

            make_popup(child.0, true);

            assert!(GetParent(child.0).is_err());
            let after = get_window_rect_safe(child.0).unwrap();
            assert_eq!(
                (after.left, after.top, after.right, after.bottom),
                expected,
                "detaching must preserve the screen rectangle"
            );
            assert_eq!(IsWindowVisible(child.0).as_bool(), initially_visible);
            assert_ne!(
                GetWindowLongW(child.0, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0,
                0
            );
            OBSERVATIONS.with(|observations| {
                let observations = observations.borrow();
                assert!(
                    !observations.is_empty(),
                    "the real window procedure must observe the transition"
                );
                for observation in observations.iter().filter(|o| o.visible && o.popup) {
                    assert_eq!(
                        observation.rect, expected,
                        "visible popup moved during message {:x}: {observations:?}",
                        observation.message
                    );
                }
            });

            OBSERVATIONS.with(|observations| observations.borrow_mut().clear());
            make_popup(child.0, false);
            assert_eq!(IsWindowVisible(child.0).as_bool(), initially_visible);
            assert_eq!(
                GetWindowLongW(child.0, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0,
                0
            );
            OBSERVATIONS.with(|observations| {
                for observation in observations.borrow().iter() {
                    assert_eq!(
                        observation.visible, initially_visible,
                        "an existing popup must not be hidden and shown again"
                    );
                    assert_eq!(observation.rect, expected);
                }
            });
        }
        drop(parent);
        UnregisterClassW(PCWSTR(class.as_ptr()), Some(instance.into())).unwrap();
    }
}

#[test]
fn shell_flyout_does_not_remove_a_still_live_taskbar_from_discovery() {
    let remembered = [11isize];
    let current_rect = rect(0, 912, 1536, 960);
    let found = recover_taskbars(Vec::new(), &remembered, |handle| {
        assert_eq!(handle, HWND(11 as *mut _));
        Some(taskbar(11, current_rect, true))
    });
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].hwnd, HWND(11 as *mut _));
    assert_eq!(found[0].rect, current_rect);
}

#[test]
fn taskbar_recovery_drops_destroyed_hosts_and_does_not_duplicate_enumerated_hosts() {
    let current = taskbar(22, rect(0, 912, 1536, 960), true);
    let found = recover_taskbars(vec![current], &[11, 22], |handle| {
        assert_ne!(
            handle, current.hwnd,
            "an enumerated host needs no fallback lookup"
        );
        None
    });
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].hwnd, current.hwnd);
}

#[test]
fn taskbar_recovery_prefers_the_enumerated_replacement_primary() {
    let current = taskbar(22, rect(0, 912, 1536, 960), true);
    let secondary = taskbar(33, rect(-1536, 912, 0, 960), false);
    let found = recover_taskbars(vec![current], &[11, 33], |handle| {
        Some(if handle == secondary.hwnd {
            secondary
        } else {
            taskbar(11, rect(0, 912, 1536, 960), true)
        })
    });
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].hwnd, current.hwnd);
    assert_eq!(found[1].hwnd, secondary.hwnd);
}

#[test]
fn taskbar_fallback_rejects_invalid_handles_and_unrelated_window_classes() {
    assert!(taskbar_window(HWND::default()).is_none());
    let class = wide_str("STATIC");
    unsafe {
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            PCWSTR(class.as_ptr()),
            PCWSTR::null(),
            WS_POPUP,
            0,
            0,
            10,
            10,
            None,
            None,
            Some(GetModuleHandleW(PCWSTR::null()).unwrap().into()),
            None,
        )
        .unwrap();
        assert!(taskbar_window(hwnd).is_none());
        DestroyWindow(hwnd).unwrap();
        assert!(taskbar_window(hwnd).is_none());
    }
}

#[test]
fn taskbar_recovery_prefers_the_enumerated_replacement_secondary() {
    let current = taskbar(22, rect(-1536, 912, 0, 960), false);
    for old_rect in [current.rect, rect(-1536, 911, 0, 959)] {
        let found = recover_taskbars(vec![current], &[11], |_| Some(taskbar(11, old_rect, false)));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].hwnd, current.hwnd);
    }
}
