fn main() {
    let payload = serde_json::json!({
        "ok": true,
        "surface": "tui",
        "status": "not_implemented",
        "message": "TUI skeleton reserved. Ratatui integration comes after workflow runtime."
    });

    println!("{}", serde_json::to_string_pretty(&payload).unwrap());
}
