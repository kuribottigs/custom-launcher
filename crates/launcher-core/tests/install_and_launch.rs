//! End-to-end test of the install → launch-command pipeline against a local
//! HTTP server (the sandbox cannot reach Mojang's CDN, and tests should not
//! depend on the network anyway).

use std::collections::HashMap;
use std::io::Write as _;
use std::sync::Arc;

use launcher_core::auth::Account;
use launcher_core::config::Paths;
use launcher_core::minecraft::install::Installer;
use launcher_core::minecraft::launch::{build_command, LaunchContext};
use launcher_core::minecraft::model::VersionJson;
use launcher_core::progress::Progress;
use sha1::{Digest, Sha1};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Serve a fixed set of paths over HTTP on an ephemeral local port.
async fn serve(files: HashMap<String, Vec<u8>>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let files = Arc::new(files);
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => break,
            };
            let files = files.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = request
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("/")
                    .to_string();
                let response = match files.get(&path) {
                    Some(body) => {
                        let mut resp = format!(
                            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                            body.len()
                        )
                        .into_bytes();
                        resp.extend_from_slice(body);
                        resp
                    }
                    None => b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                        .to_vec(),
                };
                let _ = socket.write_all(&response).await;
                let _ = socket.shutdown().await;
            });
        }
    });
    port
}

fn sha1_hex(data: &[u8]) -> String {
    hex::encode(Sha1::digest(data))
}

/// A tiny valid zip archive containing one file, for natives extraction.
fn fake_natives_jar() -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buf);
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("libtest.so", options).unwrap();
        zip.write_all(b"native!").unwrap();
        zip.start_file("META-INF/MANIFEST.MF", options).unwrap();
        zip.write_all(b"Manifest-Version: 1.0").unwrap();
        zip.finish().unwrap();
    }
    buf.into_inner()
}

#[tokio::test]
async fn installs_version_and_builds_launch_command() {
    let client_jar = b"fake client jar".to_vec();
    let library_jar = b"fake library".to_vec();
    let natives_jar = fake_natives_jar();
    let asset = b"pling.ogg contents".to_vec();
    let asset_hash = sha1_hex(&asset);

    let mut files = HashMap::new();
    files.insert("/client.jar".to_string(), client_jar.clone());
    files.insert(
        "/maven/org/example/lib/1.0/lib-1.0.jar".to_string(),
        library_jar.clone(),
    );
    files.insert(
        "/maven/org/example/nat/1.0/nat-1.0-natives-linux.jar".to_string(),
        natives_jar.clone(),
    );
    files.insert(
        format!("/{}/{}", &asset_hash[..2], asset_hash),
        asset.clone(),
    );

    let asset_index_body = format!(
        r#"{{"objects": {{"minecraft/sounds/note/pling.ogg": {{"hash": "{asset_hash}", "size": {}}}}}}}"#,
        asset.len()
    );
    files.insert("/asset-index.json".to_string(), asset_index_body.into_bytes());

    let port = serve(files).await;
    let base = format!("http://127.0.0.1:{port}");

    let version_json = format!(
        r#"{{
        "id": "1.99-test",
        "type": "release",
        "mainClass": "net.minecraft.client.main.Main",
        "assets": "99",
        "assetIndex": {{"id": "99", "url": "{base}/asset-index.json"}},
        "downloads": {{"client": {{"url": "{base}/client.jar", "sha1": "{client_sha}", "size": {client_size}}}}},
        "javaVersion": {{"component": "java-runtime-test", "majorVersion": 21}},
        "arguments": {{
            "jvm": [
                {{"rules": [{{"action": "allow", "os": {{"name": "osx"}}}}], "value": ["-XstartOnFirstThread"]}},
                "-Djava.library.path=${{natives_directory}}",
                "-cp", "${{classpath}}"
            ],
            "game": [
                "--username", "${{auth_player_name}}",
                "--version", "${{version_name}}",
                "--gameDir", "${{game_directory}}",
                "--assetsDir", "${{assets_root}}",
                "--assetIndex", "${{assets_index_name}}",
                "--uuid", "${{auth_uuid}}",
                "--accessToken", "${{auth_access_token}}",
                "--userType", "${{user_type}}",
                {{"rules": [{{"action": "allow", "features": {{"is_demo_user": true}}}}], "value": "--demo"}}
            ]
        }},
        "libraries": [
            {{
                "name": "org.example:lib:1.0",
                "downloads": {{"artifact": {{
                    "path": "org/example/lib/1.0/lib-1.0.jar",
                    "url": "{base}/maven/org/example/lib/1.0/lib-1.0.jar",
                    "sha1": "{lib_sha}"
                }}}}
            }},
            {{
                "name": "org.example:nat:1.0",
                "natives": {{"linux": "natives-linux"}},
                "extract": {{"exclude": ["META-INF/"]}},
                "downloads": {{"classifiers": {{"natives-linux": {{
                    "path": "org/example/nat/1.0/nat-1.0-natives-linux.jar",
                    "url": "{base}/maven/org/example/nat/1.0/nat-1.0-natives-linux.jar",
                    "sha1": "{nat_sha}"
                }}}}}}
            }},
            {{
                "name": "org.example:winonly:1.0",
                "rules": [{{"action": "allow", "os": {{"name": "windows"}}}}],
                "downloads": {{"artifact": {{
                    "path": "org/example/winonly/1.0/winonly-1.0.jar",
                    "url": "{base}/missing.jar"
                }}}}
            }}
        ]
    }}"#,
        client_sha = sha1_hex(&client_jar),
        client_size = client_jar.len(),
        lib_sha = sha1_hex(&library_jar),
        nat_sha = sha1_hex(&natives_jar),
    );

    let version: VersionJson = serde_json::from_str(&version_json).expect("version JSON parses");

    let temp = std::env::temp_dir().join(format!("oxide-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    let paths = Paths::new(temp.clone());
    let mut installer = Installer::new(paths.clone(), Progress::none());
    installer.resources_base = base.clone();

    let jar = installer
        .install_client_jar(&version, "1.99-test")
        .await
        .expect("client jar installs");
    assert_eq!(std::fs::read(&jar).unwrap(), b"fake client jar");

    let classpath = installer
        .install_libraries(&version, "1.99-test")
        .await
        .expect("libraries install");
    // The windows-only library must be filtered out on Linux.
    assert_eq!(classpath.len(), 1);
    assert!(classpath[0].ends_with("org/example/lib/1.0/lib-1.0.jar"));
    assert!(classpath[0].exists());

    // Natives extracted, exclusions honored.
    let natives_dir = paths.natives_dir("1.99-test");
    assert!(natives_dir.join("libtest.so").exists());
    assert!(!natives_dir.join("MANIFEST.MF").exists());

    installer.install_assets(&version).await.expect("assets install");
    let object = paths
        .assets_dir()
        .join("objects")
        .join(&asset_hash[..2])
        .join(&asset_hash);
    assert_eq!(std::fs::read(object).unwrap(), asset);

    // Build the launch command for an offline account.
    let account = Account::offline("TestSteve");
    let game_dir = temp.join("instances/test/.minecraft");
    let ctx = LaunchContext {
        paths: &paths,
        version: &version,
        jar_id: "1.99-test",
        game_dir: &game_dir,
        classpath: &classpath,
        account: &account,
        java: std::path::Path::new("/usr/bin/java"),
        memory_mb: 1024,
        extra_jvm_args: &[],
    };
    let cmd = build_command(&ctx).expect("command builds");

    let joined = cmd.join(" ");
    assert!(cmd.contains(&"net.minecraft.client.main.Main".to_string()));
    assert!(cmd.contains(&"-Xmx1024M".to_string()));
    assert!(joined.contains("--username TestSteve"));
    assert!(joined.contains("--version 1.99-test"));
    assert!(joined.contains("--userType legacy"));
    // No unresolved placeholders anywhere.
    assert!(!joined.contains("${"), "unresolved placeholder in: {joined}");
    // macOS-only JVM flag filtered out, demo-feature flag filtered out.
    assert!(!cmd.contains(&"-XstartOnFirstThread".to_string()));
    assert!(!cmd.contains(&"--demo".to_string()));
    // Classpath includes both the library and the client jar.
    let cp_index = cmd.iter().position(|c| c == "-cp").expect("-cp present");
    let cp = &cmd[cp_index + 1];
    assert!(cp.contains("lib-1.0.jar"));
    assert!(cp.contains("1.99-test.jar"));

    // Idempotency: a second install with matching hashes downloads nothing new.
    installer
        .install_client_jar(&version, "1.99-test")
        .await
        .expect("re-install is a no-op");

    let _ = std::fs::remove_dir_all(&temp);
}
