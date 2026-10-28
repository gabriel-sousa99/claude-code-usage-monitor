use super::*;

fn track_context_menu_with(
    surface: HWND,
    controller: HWND,
    track: impl FnOnce(HWND) -> usize,
) -> usize {
    // A docked surface belongs to Explorer's window hierarchy. Use our own
    // top-level controller to activate the app and own the native menu instead.
    let owner = if controller.is_invalid() {
        surface
    } else {
        controller
    };
    unsafe {
        let was_visible = IsWindowVisible(owner).as_bool();
        let was_topmost = GetWindowLongW(owner, GWL_EXSTYLE) & WS_EX_TOPMOST.0 as i32 != 0;
        // The shell's taskbar is topmost. Its child widget must not cover the
        // lower rows of a popup whose owner would otherwise be non-topmost.
        let _ = SetWindowPos(
            owner,
            Some(HWND_TOPMOST),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        if !was_visible {
            // The controller has a zero-sized client area and TOOLWINDOW style:
            // making it eligible for activation exposes no extra UI or taskbar item.
            let _ = ShowWindow(owner, SW_SHOWNOACTIVATE);
        }
        let _ = SetForegroundWindow(owner);
        let selected = track(owner);
        // Required by TrackPopupMenu's tray-menu protocol for repeated opens
        // after another shell popup has taken and released foreground activation.
        let _ = PostMessageW(Some(owner), WM_NULL, WPARAM(0), LPARAM(0));
        if !was_visible {
            let _ = ShowWindow(owner, SW_HIDE);
        }
        if !was_topmost {
            let _ = SetWindowPos(
                owner,
                Some(HWND_NOTOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        selected
    }
}

fn taskbar_at_menu_point(
    taskbars: &[native_interop::TaskbarWindow],
    point: POINT,
) -> Option<native_interop::TaskbarWindow> {
    taskbars
        .iter()
        .find(|taskbar| {
            let rect = taskbar.rect;
            point.x >= rect.left
                && point.x < rect.right
                && point.y >= rect.top
                && point.y < rect.bottom
        })
        .copied()
}

fn taskbar_menu_anchor(
    mut point: POINT,
    taskbar: RECT,
    monitor: RECT,
) -> (POINT, TRACK_POPUP_MENU_FLAGS) {
    // Align to the workspace-facing edge, not to a cursor inside the taskbar.
    // This also avoids overlap on shell composition layers above native popups.
    let alignment = if native_interop::is_taskbar_horizontal(taskbar) {
        if i64::from(taskbar.top) + i64::from(taskbar.bottom)
            <= i64::from(monitor.top) + i64::from(monitor.bottom)
        {
            point.y = taskbar.bottom;
            TPM_TOPALIGN | TPM_VERTICAL
        } else {
            point.y = taskbar.top;
            TPM_BOTTOMALIGN | TPM_VERTICAL
        }
    } else if i64::from(taskbar.left) + i64::from(taskbar.right)
        <= i64::from(monitor.left) + i64::from(monitor.right)
    {
        point.x = taskbar.right;
        TPM_LEFTALIGN
    } else {
        point.x = taskbar.left;
        TPM_RIGHTALIGN
    };
    (point, alignment)
}

pub(super) fn show_context_menu_document(
    hwnd: HWND,
    reference: Option<&str>,
    origin: Option<(usize, String)>,
) {
    let document = match context_menu::resolve_context_menu(reference) {
        Ok(document) => document,
        Err(error) => {
            diagnose::log(format!("context menu load failed: {error}"));
            context_menu::classic_context_menu()
        }
    };
    let language = lock_state()
        .as_ref()
        .map(|state| state.language)
        .unwrap_or_else(localization::detect_system_language);
    let data_context = context_menu_data_context(origin.as_ref());
    let mut actions = Vec::new();
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return;
        };
        append_context_menu_items(
            menu,
            &document.items,
            language,
            &data_context,
            origin.as_ref(),
            &mut actions,
        );
        let mut point = POINT::default();
        let _ = GetCursorPos(&mut point);
        // Mouse actions and tray callbacks can pass the primary HWND even
        // when invoked on another surface/display. Anchor only to the bar
        // under the cursor, never to that HWND's unrelated parent.
        let dock =
            taskbar_at_menu_point(&native_interop::find_taskbars(), point).and_then(|taskbar| {
                let display_handle = MonitorFromWindow(taskbar.hwnd, MONITOR_DEFAULTTONEAREST);
                let display = native_interop::find_monitors()
                    .into_iter()
                    .find(|display| display.handle == display_handle)?;
                Some((taskbar.rect, display.rect))
            });
        let (point, alignment) = dock
            .map(|(taskbar, monitor)| taskbar_menu_anchor(point, taskbar, monitor))
            .unwrap_or((point, TPM_LEFTALIGN | TPM_TOPALIGN));
        let exclusion = dock.map(|(taskbar, _)| TPMPARAMS {
            cbSize: std::mem::size_of::<TPMPARAMS>() as u32,
            rcExclude: taskbar,
        });
        let selected = track_context_menu_with(hwnd, controller_for(hwnd), |owner| {
            TrackPopupMenuEx(
                menu,
                (TPM_RIGHTBUTTON | TPM_RETURNCMD | alignment).0,
                point.x,
                point.y,
                owner,
                exclusion.as_ref().map(|params| params as *const TPMPARAMS),
            )
            .0 as usize
        });
        let _ = DestroyMenu(menu);
        if selected >= 1_000 {
            if let Some(action) = actions.get(selected - 1_000).cloned() {
                execute_context_menu_action(hwnd, action, origin);
            }
        }
    }
}

pub(super) fn context_menu_data_context(origin: Option<&(usize, String)>) -> DataContext {
    let state = lock_state();
    let Some(state) = state.as_ref() else {
        return DataContext::from_usage(None, &Canvas::default());
    };
    let mut runtime = theme_runtime_from_state(state);
    let mut canvas = Canvas::default();
    if let Some(theme) = effective_theme_from_state(state) {
        let surface_index = origin.map_or(0, |(surface_index, _)| *surface_index);
        if theme.surfaces.get(surface_index).is_some() {
            runtime = theme_runtime_for_surface(&theme, surface_index, runtime);
            let (width, height) = theme_engine::resolve_surface_content_size(
                &theme,
                surface_index,
                state.data.as_ref(),
                runtime,
            );
            canvas.width = width;
            canvas.height = height;
        }
    }
    DataContext::from_usage_with_runtime(state.data.as_ref(), &canvas, runtime)
}

unsafe fn append_context_menu_items(
    menu: HMENU,
    items: &[ContextMenuItem],
    language: LanguageId,
    context: &DataContext,
    origin: Option<&(usize, String)>,
    actions: &mut Vec<ContextMenuAction>,
) {
    for item in items {
        if !item.should_render(context) {
            continue;
        }
        match &item.kind {
            ContextMenuItemKind::Separator => {
                let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            }
            ContextMenuItemKind::Text => {
                let label = native_interop::wide_str(&context_menu::rendered_label(
                    language,
                    &item.label,
                    context,
                ));
                let _ = AppendMenuW(menu, MF_GRAYED, 0, PCWSTR::from_raw(label.as_ptr()));
            }
            ContextMenuItemKind::Submenu { items } => {
                let Ok(submenu) = CreatePopupMenu() else {
                    continue;
                };
                append_context_menu_items(submenu, items, language, context, origin, actions);
                let label = native_interop::wide_str(&context_menu::rendered_label(
                    language,
                    &item.label,
                    context,
                ));
                let _ = AppendMenuW(
                    menu,
                    MF_POPUP,
                    submenu.0 as usize,
                    PCWSTR::from_raw(label.as_ptr()),
                );
            }
            ContextMenuItemKind::Action { action } => {
                let id = 1_000 + actions.len();
                let label = native_interop::wide_str(&context_menu::rendered_label(
                    language,
                    &item.label,
                    context,
                ));
                let flags = context_menu_action_flags(action, origin);
                let _ = AppendMenuW(menu, flags, id, PCWSTR::from_raw(label.as_ptr()));
                actions.push(action.clone());
            }
        }
    }
}

pub(super) fn context_menu_action_flags(
    action: &ContextMenuAction,
    origin: Option<&(usize, String)>,
) -> MENU_ITEM_FLAGS {
    let state = lock_state();
    let Some(state) = state.as_ref() else {
        return MENU_ITEM_FLAGS(0);
    };
    let checked = match action {
        ContextMenuAction::SetUpdateFrequency { seconds } => {
            state.poll_interval_ms == seconds.saturating_mul(1_000)
        }
        ContextMenuAction::ToggleProvider { provider } => state.providers.contains(*provider),
        ContextMenuAction::ToggleAntigravityGemini => state.show_antigravity_gemini,
        ContextMenuAction::ToggleAntigravityExternal => state.show_antigravity_external,
        ContextMenuAction::ToggleStartup => is_startup_enabled(),
        ContextMenuAction::ToggleTaskbarLock => state.lock_taskbar,
        ContextMenuAction::ToggleWidget => state
            .active_theme
            .as_ref()
            .and_then(|theme| {
                let effective = theme_engine::apply_mouse_action_overrides(
                    theme,
                    &state.mouse_action_overrides,
                );
                context_menu_widget_origin(&effective).map(|(surface_index, _)| {
                    let runtime = theme_runtime_for_surface(
                        &effective,
                        surface_index,
                        theme_runtime_from_state(state),
                    );
                    theme_engine::surface_should_render(
                        &effective,
                        surface_index,
                        state.data.as_ref(),
                        runtime,
                    )
                })
            })
            .unwrap_or(false),
        ContextMenuAction::SetLanguage { language } => {
            if language.eq_ignore_ascii_case("system") {
                state.language_override.is_none()
            } else {
                state
                    .language_override
                    .is_some_and(|current| current.code().eq_ignore_ascii_case(language))
            }
        }
        ContextMenuAction::ToggleLayerRender { target } => state
            .active_theme
            .as_ref()
            .and_then(|theme| {
                let effective = theme_engine::apply_mouse_action_overrides(
                    theme,
                    &state.mouse_action_overrides,
                );
                effective
                    .surfaces
                    .iter()
                    .position(|surface| surface.id.eq_ignore_ascii_case(target))
                    .map(|surface_index| {
                        let runtime = theme_runtime_for_surface(
                            &effective,
                            surface_index,
                            theme_runtime_from_state(state),
                        );
                        theme_engine::surface_should_render(
                            &effective,
                            surface_index,
                            state.data.as_ref(),
                            runtime,
                        )
                    })
            })
            .unwrap_or(false),
        _ => false,
    };
    let disabled = matches!(
        action,
        ContextMenuAction::CheckForUpdates
            if matches!(state.update_status, UpdateStatus::Checking | UpdateStatus::Applying)
    ) || matches!(
        action,
        ContextMenuAction::LayerActions { .. } | ContextMenuAction::ToggleLayerRender { .. }
    ) && origin.is_none()
        && state.active_theme.is_none();
    match (checked, disabled) {
        (true, true) => MF_CHECKED | MF_GRAYED,
        (true, false) => MF_CHECKED,
        (false, true) => MF_GRAYED,
        (false, false) => MENU_ITEM_FLAGS(0),
    }
}

pub(super) fn context_menu_action_origin(
    origin: Option<(usize, String)>,
) -> Option<(usize, String)> {
    if origin.is_some() {
        return origin;
    }
    lock_state().as_ref().and_then(|state| {
        state
            .active_theme
            .as_ref()
            .and_then(|theme| theme.surfaces.first())
            .map(|surface| (0, surface.id.clone()))
    })
}

pub(super) fn context_menu_widget_origin(theme: &ThemeDocument) -> Option<(usize, String)> {
    theme
        .surfaces
        .iter()
        .enumerate()
        .find(|(_, surface)| surface.placement.nest != SurfaceNest::TrayIcon)
        .map(|(index, surface)| (index, surface.id.clone()))
}

pub(super) fn execute_context_menu_action(
    hwnd: HWND,
    action: ContextMenuAction,
    origin: Option<(usize, String)>,
) {
    let static_command = match &action {
        ContextMenuAction::OpenDashboard => Some(IDM_DASHBOARD),
        ContextMenuAction::Refresh => Some(1),
        ContextMenuAction::SetUpdateFrequency { seconds } => match *seconds {
            POLL_1_MIN_SECONDS => Some(IDM_FREQ_1MIN),
            POLL_5_MIN_SECONDS => Some(IDM_FREQ_5MIN),
            POLL_15_MIN_SECONDS => Some(IDM_FREQ_15MIN),
            POLL_1_HOUR_SECONDS => Some(IDM_FREQ_1HOUR),
            _ => None,
        },
        ContextMenuAction::ToggleProvider { provider } => {
            Some(provider.descriptor().native_menu_command_id)
        }
        ContextMenuAction::ToggleAntigravityGemini => Some(IDM_PROVIDER_ANTIGRAVITY_GEMINI),
        ContextMenuAction::ToggleAntigravityExternal => Some(IDM_PROVIDER_ANTIGRAVITY_EXTERNAL),
        ContextMenuAction::ToggleStartup => Some(IDM_START_WITH_WINDOWS),
        ContextMenuAction::ToggleTaskbarLock => Some(IDM_LOCK_TASKBAR),
        ContextMenuAction::SetLanguage { language } => {
            if language.eq_ignore_ascii_case("system") {
                Some(IDM_LANG_SYSTEM)
            } else {
                LanguageId::from_code(language).map(language_menu_command_id)
            }
        }
        ContextMenuAction::CheckForUpdates => Some(IDM_VERSION_ACTION),
        ContextMenuAction::Exit => Some(2),
        ContextMenuAction::ToggleWidget
        | ContextMenuAction::LegacyResetPosition
        | ContextMenuAction::ToggleLayerRender { .. }
        | ContextMenuAction::LayerActions { .. }
        | ContextMenuAction::OpenUrl { .. } => None,
    };
    if let Some(command) = static_command {
        unsafe {
            let _ = PostMessageW(
                Some(controller_for(hwnd)),
                WM_COMMAND,
                WPARAM(command as usize),
                LPARAM(0),
            );
        }
        return;
    }
    match action {
        ContextMenuAction::ToggleWidget => {
            let target = lock_state()
                .as_ref()
                .and_then(|state| state.active_theme.as_ref())
                .and_then(context_menu_widget_origin);
            if let Some((surface_index, root_id)) = target {
                let _ =
                    execute_mouse_action_source(surface_index, &root_id, "toggle(self, render)");
            }
        }
        ContextMenuAction::ToggleLayerRender { target } => {
            let Some((surface_index, self_id)) = context_menu_action_origin(origin) else {
                return;
            };
            let target = target.replace('\\', "\\\\").replace('"', "\\\"");
            let source = format!("toggle(\"{target}\", render)");
            let _ = execute_mouse_action_source(surface_index, &self_id, &source);
        }
        ContextMenuAction::LayerActions { actions } => {
            let Some((surface_index, self_id)) = context_menu_action_origin(origin) else {
                return;
            };
            let _ = execute_mouse_action_source(surface_index, &self_id, &actions);
        }
        ContextMenuAction::OpenUrl { url } => {
            open_web_url(hwnd, &url, "context menu URL could not be opened")
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::UI::WindowsAndMessaging::{GetMenuItemCount, GetMenuItemID, GetSubMenu};

    #[test]
    fn menu_placement_uses_the_clicked_taskbar_and_leaves_floating_surfaces_at_the_cursor() {
        let bars = [
            native_interop::TaskbarWindow {
                hwnd: HWND(11 as *mut _),
                rect: RECT {
                    left: 0,
                    top: 1032,
                    right: 1920,
                    bottom: 1080,
                },
                primary: true,
            },
            native_interop::TaskbarWindow {
                hwnd: HWND(22 as *mut _),
                rect: RECT {
                    left: 1920,
                    top: 1032,
                    right: 3840,
                    bottom: 1080,
                },
                primary: false,
            },
        ];
        assert!(
            taskbar_at_menu_point(&bars, POINT { x: 2200, y: 800 }).is_none(),
            "a floating secondary surface must not inherit the primary taskbar"
        );
        assert_eq!(
            taskbar_at_menu_point(&bars, POINT { x: 3500, y: 1060 })
                .unwrap()
                .hwnd,
            bars[1].hwnd,
            "a secondary tray callback must use the taskbar under its cursor"
        );
        assert!(taskbar_at_menu_point(&bars, POINT { x: 1920, y: 1080 }).is_none());
    }

    #[test]
    fn taskbar_menu_anchor_keeps_the_menu_outside_each_taskbar_edge() {
        let rect = |left, top, right, bottom| RECT {
            left,
            top,
            right,
            bottom,
        };
        for scale in [1, 2] {
            let monitor = rect(-1920 * scale, -1080 * scale, 0, 0);
            for (bar, cursor, expected, alignment) in [
                (
                    rect(-1920, -48, 0, 0),
                    (-600, -20),
                    (-600, -48),
                    TPM_BOTTOMALIGN,
                ),
                (
                    rect(-1920, -1080, 0, -1032),
                    (-600, -1060),
                    (-600, -1032),
                    TPM_TOPALIGN,
                ),
                (
                    rect(-1920, -1080, -1872, 0),
                    (-1900, -500),
                    (-1872, -500),
                    TPM_LEFTALIGN,
                ),
                (
                    rect(-48, -1080, 0, 0),
                    (-20, -500),
                    (-48, -500),
                    TPM_RIGHTALIGN,
                ),
            ] {
                let bar = rect(
                    bar.left * scale,
                    bar.top * scale,
                    bar.right * scale,
                    bar.bottom * scale,
                );
                let (point, flags) = taskbar_menu_anchor(
                    POINT {
                        x: cursor.0 * scale,
                        y: cursor.1 * scale,
                    },
                    bar,
                    monitor,
                );
                assert_eq!((point.x, point.y), (expected.0 * scale, expected.1 * scale));
                assert_eq!(flags.0 & (TPM_BOTTOMALIGN | TPM_RIGHTALIGN).0, alignment.0);
            }
        }
    }

    #[test]
    fn docked_context_menus_use_the_apps_top_level_owner_and_restore_its_visibility() {
        unsafe {
            let class = native_interop::wide_str("STATIC");
            let create = |style, parent| {
                CreateWindowExW(
                    WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    PCWSTR(class.as_ptr()),
                    PCWSTR::null(),
                    style,
                    0,
                    0,
                    0,
                    0,
                    parent,
                    None,
                    None,
                    None,
                )
                .unwrap()
            };
            let shell = create(WS_POPUP, None);
            let surface = create(WS_CHILD, Some(shell));
            let controller = create(WS_POPUP, None);
            for expected_selection in [0, 1001, 0] {
                let mut observed_owner = HWND::default();
                let mut visible_during_tracking = false;
                let mut topmost_during_tracking = false;
                let selected = track_context_menu_with(surface, controller, |owner| {
                    observed_owner = owner;
                    visible_during_tracking = IsWindowVisible(owner).as_bool();
                    topmost_during_tracking =
                        GetWindowLongW(owner, GWL_EXSTYLE) & WS_EX_TOPMOST.0 as i32 != 0;
                    expected_selection
                });
                // Collect before asserting so failing regressions still release windows.
                let top_level = GetParent(observed_owner).is_err();
                let hidden_after = !IsWindowVisible(controller).as_bool();
                let topmost_after =
                    GetWindowLongW(controller, GWL_EXSTYLE) & WS_EX_TOPMOST.0 as i32 != 0;
                if observed_owner != controller
                    || !top_level
                    || !visible_during_tracking
                    || !topmost_during_tracking
                    || topmost_after
                    || !hidden_after
                    || selected != expected_selection
                {
                    let _ = DestroyWindow(surface);
                    let _ = DestroyWindow(shell);
                    let _ = DestroyWindow(controller);
                }
                assert_eq!(
                    observed_owner, controller,
                    "the shell-owned child must not own the native popup"
                );
                assert!(top_level);
                assert!(visible_during_tracking);
                assert!(
                    topmost_during_tracking,
                    "the popup owner must rank above the topmost taskbar"
                );
                assert!(
                    !topmost_after,
                    "temporary menu z-order must not leak to the controller"
                );
                assert!(hidden_after);
                assert_eq!(selected, expected_selection);
            }
            DestroyWindow(surface).unwrap();
            DestroyWindow(shell).unwrap();
            DestroyWindow(controller).unwrap();
        }
    }

    #[test]
    fn conditional_native_rows_and_subtrees_preserve_action_ids_on_each_open() {
        let mut conditional = vec![
            ContextMenuItem::text("usage", "Weekly usage"),
            ContextMenuItem::separator("divider"),
            ContextMenuItem::action("refresh", "Refresh", ContextMenuAction::Refresh),
            ContextMenuItem::submenu(
                "provider",
                "Provider",
                vec![ContextMenuItem::action(
                    "nested",
                    "Exit",
                    ContextMenuAction::Exit,
                )],
            ),
        ];
        for item in &mut conditional {
            item.render = theme_engine::Expression("providers.codex.enabled".into());
        }
        // This row stays hidden even when its parent becomes visible.
        if let ContextMenuItemKind::Submenu { items } = &mut conditional[3].kind {
            let mut hidden = ContextMenuItem::text("hidden", "Hidden child");
            hidden.render = theme_engine::Expression("0".into());
            items.insert(0, hidden);
        }
        conditional.push(ContextMenuItem::action(
            "exit",
            "Exit",
            ContextMenuAction::Exit,
        ));
        let mut context = DataContext::from_usage(None, &Canvas::default());
        for enabled in [false, true, false] {
            context.insert("providers.codex.enabled", enabled as u8 as f64);
            let mut actions = Vec::new();
            unsafe {
                let menu = CreatePopupMenu().unwrap();
                append_context_menu_items(
                    menu,
                    &conditional,
                    localization::detect_system_language(),
                    &context,
                    None,
                    &mut actions,
                );
                let count = GetMenuItemCount(Some(menu));
                let last_id = GetMenuItemID(menu, count - 1);
                let child_count = if enabled {
                    let submenu = GetSubMenu(menu, 3);
                    Some((GetMenuItemCount(Some(submenu)), GetMenuItemID(submenu, 0)))
                } else {
                    None
                };
                DestroyMenu(menu).unwrap();
                assert_eq!(count, if enabled { 5 } else { 1 });
                assert_eq!(last_id, if enabled { 1002 } else { 1000 });
                if enabled {
                    assert_eq!(child_count, Some((1, 1001)));
                    assert_eq!(
                        actions,
                        vec![
                            ContextMenuAction::Refresh,
                            ContextMenuAction::Exit,
                            ContextMenuAction::Exit
                        ]
                    );
                } else {
                    assert_eq!(actions, vec![ContextMenuAction::Exit]);
                }
            }
        }
    }
}
