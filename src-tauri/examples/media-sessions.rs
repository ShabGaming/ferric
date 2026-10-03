use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager;

fn main() -> windows::core::Result<()> {
    let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.join()?;
    let sessions = manager.GetSessions()?;
    for index in 0..sessions.Size()? {
        println!("{}", sessions.GetAt(index)?.SourceAppUserModelId()?);
    }
    Ok(())
}
