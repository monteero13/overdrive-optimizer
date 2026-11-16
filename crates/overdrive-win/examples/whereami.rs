//! Diagnostics: what does the service see for running processes? (read-only)
fn main() {
    let filter = std::env::args().nth(1).unwrap_or_default().to_ascii_lowercase();
    for p in overdrive_win::processes::list().unwrap() {
        if !filter.is_empty() && !p.process.image.to_ascii_lowercase().contains(&filter) { continue; }
        let path = overdrive_win::processes::image_path(p.process.pid);
        println!("{:>6} s{} win={} fg={} audio={} {:<28} {:?}", p.process.pid, p.session_id, p.has_visible_window, p.is_foreground, p.produces_audio, p.process.image, path);
    }
}
