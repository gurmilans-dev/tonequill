use tonequill_live::events::*;
use ts_rs::{Config, TS};
fn main() -> anyhow::Result<()> {
    let cfg = Config::default();
    let declarations = [
        Direction::decl(&cfg),
        TransferMode::decl(&cfg),
        Phase::decl(&cfg),
        FileInfo::decl(&cfg),
        Progress::decl(&cfg),
        SignalMetrics::decl(&cfg),
        FrameMetrics::decl(&cfg),
        ErrorCode::decl(&cfg),
        Failure::decl(&cfg),
        Completion::decl(&cfg),
        SessionStatus::decl(&cfg),
        Event::decl(&cfg),
        SessionEvent::decl(&cfg),
        SessionSnapshot::decl(&cfg),
        SessionUpdate::decl(&cfg),
        DeviceSelection::decl(&cfg),
        DeviceInfo::decl(&cfg),
        SessionRequest::decl(&cfg),
    ];
    let text = format!(
        "// Generated from tonequill-live Rust types. Do not edit.\n{}\n",
        declarations
            .iter()
            .map(|s| format!("export {s}"))
            .collect::<Vec<_>>()
            .join("\n\n")
    );
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "apps/desktop/src/domain/generated.ts".into());
    let path = std::path::Path::new(&path);
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(path, text)?;
    Ok(())
}
