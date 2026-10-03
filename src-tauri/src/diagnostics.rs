use std::{
    fs::OpenOptions,
    io::Write,
    sync::mpsc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::{Value, json};
use tauri::{Manager, WebviewWindow};
use webview2_com::CallDevToolsProtocolMethodCompletedHandler;
use windows_core::PCWSTR;

pub fn start(window: WebviewWindow) {
    std::thread::spawn(move || {
        if let Err(error) = record(&window) {
            eprintln!("Could not record page-memory diagnostics: {error}");
        }
    });
}

fn record(window: &WebviewWindow) -> Result<(), Box<dyn std::error::Error>> {
    let directory = window
        .state::<crate::AppState>()
        .data_directory
        .join("diagnostics");
    std::fs::create_dir_all(&directory)?;
    let report = directory.join("page-memory.jsonl");
    let mut output = OpenOptions::new().create(true).append(true).open(report)?;

    protocol(window, "Performance.enable")?;
    for _ in 0..3 {
        std::thread::sleep(Duration::from_secs(30));
        let response: Value = serde_json::from_str(&protocol(window, "Performance.getMetrics")?)?;
        let metrics: Vec<Value> = response["metrics"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|metric| {
                metric["value"].is_number()
                    && matches!(
                        metric["name"].as_str(),
                        Some(
                            "JSHeapUsedSize"
                                | "JSHeapTotalSize"
                                | "Nodes"
                                | "Documents"
                                | "Frames"
                                | "LayoutCount"
                                | "RecalcStyleCount"
                                | "TaskDuration"
                        )
                    )
            })
            .map(|metric| json!({ "name": metric["name"], "value": metric["value"] }))
            .collect();
        let sample = json!({
            "timestampUnixSeconds": SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
            "processId": std::process::id(),
            "focused": window.is_focused()?,
            "minimized": window.is_minimized()?,
            "metrics": metrics,
        });
        writeln!(output, "{sample}")?;
    }
    protocol(window, "Performance.disable")?;
    Ok(())
}

fn protocol(window: &WebviewWindow, method: &'static str) -> Result<String, String> {
    let (sender, receiver) = mpsc::sync_channel(1);
    window
        .with_webview(move |webview| {
            let completion = sender.clone();
            let handler = CallDevToolsProtocolMethodCompletedHandler::create(Box::new(
                move |result, response| {
                    let _ = completion.send(result.map(|()| response).map_err(|e| e.to_string()));
                    Ok(())
                },
            ));
            let method: Vec<u16> = method.encode_utf16().chain(Some(0)).collect();
            // I call the browser protocol on the UI thread without opening a debugging port.
            let result = unsafe {
                webview.controller().CoreWebView2().and_then(|core| {
                    core.CallDevToolsProtocolMethod(
                        PCWSTR(method.as_ptr()),
                        windows_core::w!("{}"),
                        &handler,
                    )
                })
            };
            if let Err(error) = result {
                let _ = sender.send(Err(error.to_string()));
            }
        })
        .map_err(|error| error.to_string())?;
    receiver
        .recv_timeout(Duration::from_secs(5))
        .map_err(|error| error.to_string())?
}
