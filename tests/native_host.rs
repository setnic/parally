use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[test]
fn actual_executable_relays_acceptance_and_rejection_over_native_messaging() {
    let temporary =
        std::env::temp_dir().join(format!("parally-native-test-{}", std::process::id()));
    let root = if cfg!(target_os = "macos") {
        temporary.join("Library/Application Support/Parally")
    } else {
        temporary.join("Parally")
    };
    let inbox = root.join("inbox");
    fs::create_dir_all(&inbox).unwrap();
    let lock = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(root.join("instance.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    for (manual, accepted) in [(false, false), (false, true), (true, true)] {
        let mut host = Command::new(env!("CARGO_BIN_EXE_parallel-downloader"))
            .arg("chrome-extension://abcdefghijklmnopabcdefghijklmnop/")
            .env("LOCALAPPDATA", &temporary)
            .env("XDG_DATA_HOME", &temporary)
            .env("HOME", &temporary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = host.stdin.take().unwrap();
        let mut output = host.stdout.take().unwrap();
        let offer = if manual {
            json!({ "url": "http://127.0.0.1/sample.zip?direct=1", "filename":"", "manual":true })
        } else {
            json!({
                "url": "http://127.0.0.1/sample.zip?sig=1", "filename":"sample.zip",
                "referrer": "http://127.0.0.1/page", "cookie": "session=abc"
            })
        };
        let message = serde_json::to_vec(&offer).unwrap();
        input
            .write_all(&(message.len() as u32).to_le_bytes())
            .unwrap();
        input.write_all(&message).unwrap();
        input.flush().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let offer = loop {
            let offer = fs::read_dir(&inbox)
                .unwrap()
                .flatten()
                .map(|entry| entry.path())
                .find(|path| {
                    path.extension()
                        .is_some_and(|extension| extension == "offer")
                });
            if let Some(offer) = offer {
                break offer;
            }
            assert!(
                Instant::now() < deadline && host.try_wait().unwrap().is_none(),
                "The native host did not publish an offer."
            );
            thread::sleep(Duration::from_millis(20));
        };
        assert!(host.try_wait().unwrap().is_none());
        let published: Value = serde_json::from_slice(&fs::read(&offer).unwrap()).unwrap();
        assert_eq!(published["offer"].get("manual").is_some(), manual);
        assert_eq!(published["offer"].get("cookie").is_some(), !manual);
        let response = offer.with_extension("reply");
        let pending = response.with_extension("reply.new");
        fs::write(
            &pending,
            serde_json::to_vec(&json!({ "accepted": accepted })).unwrap(),
        )
        .unwrap();
        fs::rename(&pending, &response).unwrap();
        while host.try_wait().unwrap().is_none() {
            assert!(
                Instant::now() < deadline,
                "The native host did not acknowledge the response."
            );
            thread::sleep(Duration::from_millis(20));
        }
        let mut length = [0; 4];
        output.read_exact(&mut length).unwrap();
        let mut frame = vec![0; u32::from_le_bytes(length) as usize];
        output.read_exact(&mut frame).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&frame).unwrap()["accepted"],
            accepted
        );
        assert!(!offer.exists() && !response.exists());
    }
    drop(lock);
    fs::remove_dir_all(temporary).unwrap();
}
