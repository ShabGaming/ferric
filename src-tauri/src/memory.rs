use tauri::WebviewWindow;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
    ICoreWebView2_19,
};
use windows_core::Interface;

pub fn set_background(window: &WebviewWindow, background: bool) {
    let target = if background {
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
    } else {
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
    };

    let queued = window.with_webview(move |webview| {
        // I use the controller only in Tauri's UI-thread callback.
        let result = unsafe {
            webview.controller().CoreWebView2().and_then(|core| {
                core.cast::<ICoreWebView2_19>()?
                    .SetMemoryUsageTargetLevel(target)
            })
        };
        if let Err(error) = result {
            eprintln!("Could not change WebView2's memory target: {error}");
        }
    });

    if let Err(error) = queued {
        eprintln!("Could not schedule the memory target change: {error}");
    }
}
