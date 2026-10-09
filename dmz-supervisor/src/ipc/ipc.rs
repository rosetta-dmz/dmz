use tokio::net::{UnixListener, UnixStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use std::path::Path;
use std::fs;

pub struct SupervisorServer {
    socket_path: String,
}

impl SupervisorServer {
    pub fn new(socket_path: &str) -> Self {
        Self { socket_path: socket_path.to_string() }
    }

    pub async fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
        if Path::new(&self.socket_path).exists() {
            fs::remove_file(&self.socket_path)?;
        }

        let listener = UnixListener::bind(&self.socket_path)?;
        println!("[DMZ Supervisor] IPC socket active at {}", self.socket_path);

        loop {
            let (mut stream, _) = listener.accept().await?;
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                if let Ok(n) = stream.read(&mut buf).await {
                    let request = String::from_utf8_lossy(&buf[..n]);
                    let response = format!("{{\"status\":\"ok\",\"received\":\"{}\"}}", request.trim());
                    let _ = stream.write_all(response.as_bytes()).await;
                }
            });
        }
    }
}