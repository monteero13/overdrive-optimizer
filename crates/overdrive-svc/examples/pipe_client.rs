//! Minimal overdrive.v1 client for manual checks:
//!   cargo run -p overdrive-svc --example pipe_client -- [pipe] method [json-params] ...
//! Sends `hello` first, then each method in order, printing every reply.

use std::time::Duration;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let pipe = if args.first().is_some_and(|a| a.starts_with(r"\\")) {
        args.remove(0)
    } else {
        overdrive_ipc::PIPE_NAME.to_string()
    };
    let p = overdrive_ipc::connect(&pipe, Duration::from_secs(5)).expect("connect");
    let mut calls = vec![("hello".to_string(), r#"{"uiVersion":"cli","locale":"es"}"#.to_string())];
    let mut i = 0;
    while i < args.len() {
        let m = args[i].clone();
        let params = args.get(i + 1).filter(|a| a.starts_with('{')).cloned();
        i += if params.is_some() { 2 } else { 1 };
        calls.push((m, params.unwrap_or_else(|| "{}".into())));
    }
    for (n, (m, params)) in calls.iter().enumerate() {
        let id = n + 1;
        p.write_frame(&format!(r#"{{"kind":"req","id":{id},"method":"{m}","params":{params}}}"#)).unwrap();
        loop {
            let f = p.read_frame().expect("read");
            let v: serde_json::Value = serde_json::from_str(&f).unwrap();
            if v["kind"] == "res" && v["id"] == id {
                println!("{m} => {}", serde_json::to_string_pretty(&v).unwrap());
                break;
            }
        }
    }
}
