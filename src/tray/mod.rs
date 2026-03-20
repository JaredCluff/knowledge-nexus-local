//! System tray icon for desktop integration.
//!
//! Provides a menu bar icon with status indicator and menu.
//! On macOS, uses a template icon for proper dark/light mode support.
//! Requires the native NSApplication event loop on the main thread.

use anyhow::Result;
use tracing::info;

use crate::config::Config;

/// Embedded tray icon (44x44 black template PNG)
#[cfg(feature = "tray")]
const TRAY_ICON_BYTES: &[u8] = include_bytes!("tray-icon.png");

/// Start the tray icon on the main thread with the native macOS event loop.
///
/// `agent_main` is spawned on a background thread while the main thread
/// runs the native event loop required for the menu bar icon to appear.
#[cfg(feature = "tray")]
pub fn run_with_tray(config: Config, agent_main: impl FnOnce() + Send + 'static) -> Result<()> {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::TrayIconBuilder;

    info!("Initializing menu bar icon...");

    // On macOS, initialize NSApplication as an accessory (menu bar only, no dock icon)
    // SAFETY: NSApp() returns the global NSApplication singleton, guaranteed to be
    // non-null after the process starts. setActivationPolicy_ and msg_send![_, finishLaunching]
    // are well-defined Objective-C messages on NSApplication. The objc crate's msg_send!
    // macro handles selector dispatch correctly. finishLaunching is idempotent and must
    // be called once before the event loop can process menu bar events.
    #[cfg(target_os = "macos")]
    #[allow(deprecated)]
    unsafe {
        use cocoa::appkit::{NSApp, NSApplication, NSApplicationActivationPolicy};
        use objc::msg_send;
        use objc::sel;
        use objc::sel_impl;
        let app = NSApp();
        app.setActivationPolicy_(
            NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory,
        );
        // finishLaunching is required for the status bar (tray) to render
        let () = msg_send![app, finishLaunching];
    }

    // Decode the embedded PNG to get raw RGBA pixels
    let icon = load_icon()?;

    // Create menu
    let menu = Menu::new();

    let status_item = MenuItem::new(
        format!("Knowledge Nexus Agent v{}", env!("CARGO_PKG_VERSION")),
        false,
        None,
    );
    let device_item = MenuItem::new(
        format!("Device: {} ({})", config.device.name, &config.device.id[..config.device.id.len().min(12)]),
        false,
        None,
    );
    let connection_item = MenuItem::new(
        format!(
            "Hub: {}",
            config
                .connection
                .hub_url
                .as_deref()
                .unwrap_or("Not configured")
        ),
        false,
        None,
    );

    menu.append(&status_item)?;
    menu.append(&device_item)?;
    menu.append(&connection_item)?;
    menu.append(&PredefinedMenuItem::separator())?;

    // Indexed paths section
    let paths_header = MenuItem::new("Indexed Paths:", false, None);
    menu.append(&paths_header)?;
    if config.security.allowed_paths.is_empty() {
        menu.append(&MenuItem::new("  (none)", false, None))?;
    } else {
        for path in &config.security.allowed_paths {
            let exists = std::path::Path::new(path).exists();
            let label = if exists {
                format!("  {}", path)
            } else {
                format!("  {} (missing)", path)
            };
            menu.append(&MenuItem::new(label, false, None))?;
        }
    }

    menu.append(&PredefinedMenuItem::separator())?;

    let ui_item = MenuItem::new("Open Search UI", config.ui.enabled, None);
    let quit_item = MenuItem::new("Quit", true, None);

    menu.append(&ui_item)?;
    menu.append(&PredefinedMenuItem::separator())?;
    menu.append(&quit_item)?;

    let ui_item_id = ui_item.id().clone();
    let quit_item_id = quit_item.id().clone();

    // Build tray icon
    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Knowledge Nexus Agent")
        .with_icon(icon)
        .with_icon_as_template(true)
        .build()?;

    info!("Menu bar icon ready");

    // Spawn agent services on a background thread
    std::thread::spawn(agent_main);

    // Handle menu events in a background thread
    let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel::<()>();
    let ui_url = format!("http://localhost:{}", config.ui.port);
    std::thread::spawn(move || loop {
        match MenuEvent::receiver().recv() {
            Ok(event) => {
                if event.id == ui_item_id {
                    info!("Opening Search UI...");
                    let _ = open::that(&ui_url);
                } else if event.id == quit_item_id {
                    info!("Quit requested from menu bar");
                    let _ = shutdown_tx.send(());
                    return;
                }
            }
            Err(_) => return,
        }
    });

    // Run the native macOS event loop on the main thread.
    // We use nextEventMatchingMask / sendEvent to pump the Cocoa event loop
    // (required for NSStatusBar / tray icon rendering) while checking for
    // shutdown between iterations.
    #[cfg(target_os = "macos")]
    {
        // SAFETY: NSApp() returns the global NSApplication singleton, guaranteed
        // non-null after finishLaunching above. `msg_send!` handles ObjC dispatch
        // correctly for both class and instance methods. dateWithTimeIntervalSinceNow
        // returns an autoreleased NSDate (valid within this autorelease pool scope).
        // nextEventMatchingMask:untilDate:inMode:dequeue: is the standard Cocoa event
        // pump; it returns nil on timeout, which is guarded by the nil check before
        // sendEvent_. libc::getpid() is always valid; SIGTERM on the current process
        // is the correct way to trigger our signal handler for a clean shutdown.
        #[allow(deprecated)]
        unsafe {
            use cocoa::appkit::{NSApp, NSApplication};
            use cocoa::base::nil;
            use objc::msg_send;
            use objc::sel;
            use objc::sel_impl;

            let app = NSApp();
            // NSDate class for creating timeout dates
            let nsdate_class = objc::class!(NSDate);

            loop {
                // Poll for the next Cocoa event with a 1-second timeout
                // NSAnyEventMask = NSUIntegerMax
                let date: cocoa::base::id =
                    msg_send![nsdate_class, dateWithTimeIntervalSinceNow: 1.0f64];
                let event: cocoa::base::id = msg_send![
                    app,
                    nextEventMatchingMask: u64::MAX
                    untilDate: date
                    inMode: cocoa::foundation::NSDefaultRunLoopMode
                    dequeue: true
                ];
                if event != nil {
                    app.sendEvent_(event);
                }
                // Check if quit was requested
                if shutdown_rx.try_recv().is_ok() {
                    info!("Shutting down from menu bar quit...");
                    libc::kill(libc::getpid(), libc::SIGTERM);
                    break;
                }
            }
        }
    }

    // Windows: pump the Win32 message loop so tray icon responds to clicks
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
        };
        loop {
            // SAFETY: PeekMessageW/TranslateMessage/DispatchMessageW are standard
            // Win32 message pump calls. MSG is zero-initialized which is valid.
            unsafe {
                let mut msg: MSG = std::mem::zeroed();
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            if shutdown_rx.try_recv().is_ok() {
                info!("Shutting down from menu bar quit...");
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }

    // Linux fallback: block on menu events
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = shutdown_rx.recv();
        info!("Shutting down from menu bar quit...");
    }

    Ok(())
}

#[cfg(feature = "tray")]
fn load_icon() -> Result<tray_icon::Icon> {
    let decoder = png::Decoder::new(std::io::Cursor::new(TRAY_ICON_BYTES));
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf)?;
    let rgba = buf[..info.buffer_size()].to_vec();
    let icon = tray_icon::Icon::from_rgba(rgba, info.width, info.height)
        .map_err(|e| anyhow::anyhow!("Failed to create tray icon: {}", e))?;
    Ok(icon)
}

/// No-op when tray feature is disabled — just runs the agent directly.
#[cfg(not(feature = "tray"))]
pub fn run_with_tray(_config: Config, agent_main: impl FnOnce() + Send + 'static) -> Result<()> {
    info!("System tray support not compiled in");
    agent_main();
    Ok(())
}
