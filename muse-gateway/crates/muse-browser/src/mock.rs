//! Driver mock — port `MockDriver` (`Drivers.cs`, dari Python
//! `drivers/mock.py`). Offline, deterministik, tanpa akun.

use std::collections::HashMap;

use async_trait::async_trait;
use futures_util::stream::{self};

use crate::driver::{ChatStream, MuseDriver};
use crate::error::MuseError;
use crate::types::{
    Account, ChatEvent, ChatRequest, DriverCapabilities, ImageRequest, MediaResult, SessionInfo,
    VideoRequest,
};

/// Driver mock. Balasan chat persis .NET (`Drivers.cs:57` + baris
/// berikutnya):
/// `"[mock:{model}] You said: {200 karakter terakhir prompt}"`
/// ditambah `" (with {n} image(s))"` bila ada gambar — dipecah jadi delta
/// 8 karakter dengan jeda 1 ms (seperti `.NET: yield return
/// reply.Substring(i, 8)` + `Task.Delay(_delayMs)`).
///
/// Perbedaan karakter: .NET memotong 200 **UTF-16 code unit**
/// (`req.Prompt[^200..]`); Rust memotong 200 **char** (Unicode scalar).
/// Untuk teks ASCII/BMP hasilnya identik; untuk emoji di luar BMP, batas
/// 200 bisa bergeser ±beberapa karakter — lihat `TRADEOFFS.md`.
pub struct MockDriver {
    delay_ms: u64,
}

impl MockDriver {
    pub fn new() -> Self {
        Self { delay_ms: 1 }
    }

    pub fn with_delay(delay_ms: u64) -> Self {
        Self { delay_ms }
    }

    /// Balasan lengkap mock untuk satu request (tanpa chunking) — dipakai
    /// test + dokumentasi.
    pub fn reply_for(req: &ChatRequest) -> String {
        let tail: String = req.prompt.chars().rev().take(200).collect::<String>();
        let tail: String = tail.chars().rev().collect();
        let mut reply = format!("[mock:{}] You said: {tail}", req.model);
        if !req.images.is_empty() {
            reply += &format!(" (with {} image(s))", req.images.len());
        }
        reply
    }
}

impl Default for MockDriver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MuseDriver for MockDriver {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn requires_account(&self) -> bool {
        false
    }

    fn capabilities(&self) -> DriverCapabilities {
        DriverCapabilities::ALL
    }

    fn chat_stream(&self, _account: Account, req: ChatRequest) -> ChatStream {
        let reply = Self::reply_for(&req);
        let delay_ms = self.delay_ms;
        // Chunk 8 karakter (seperti .NET Substring 8) + delay 1 ms.
        let chunks: Vec<String> = reply
            .chars()
            .collect::<Vec<char>>()
            .chunks(8)
            .map(|c| c.iter().collect())
            .collect();
        let stream = stream::unfold(
            (chunks.into_iter(), false),
            move |(mut it, mut done_sent)| async move {
                if let Some(chunk) = it.next() {
                    if delay_ms > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    }
                    Some((Ok(ChatEvent::AssistantDelta(chunk)), (it, done_sent)))
                } else if !done_sent {
                    done_sent = true;
                    Some((Ok(ChatEvent::Done), (it, done_sent)))
                } else {
                    None
                }
            },
        );
        Box::pin(stream)
    }

    async fn generate_image(
        &self,
        _account: Account,
        req: ImageRequest,
    ) -> Result<Vec<MediaResult>, MuseError> {
        let n = req.n.max(1);
        let mut results = Vec::with_capacity(n);
        for i in 0..n {
            if self.delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
            }
            let color = (
                (40u16 + (50 * i as u16) % 200) as u8,
                120u8,
                200u8,
            );
            results.push(MediaResult {
                data: solid_png(64, 64, color),
                mime: "image/png".to_string(),
                kind: "image".to_string(),
                width: Some(64),
                height: Some(64),
                revised_prompt: Some(req.prompt.clone()),
            });
        }
        Ok(results)
    }

    async fn generate_video(
        &self,
        _account: Account,
        _req: VideoRequest,
    ) -> Result<MediaResult, MuseError> {
        // Placeholder bytes dengan mime yang benar, seperti mock Python/.NET.
        Ok(MediaResult {
            data: vec![
                0, 0, 0, 0x18, b'f', b't', b'y', b'p', b'm', b'p', b'4', b'2', b'm', b'o', b'c',
                b'k',
            ],
            mime: "video/mp4".to_string(),
            kind: "video".to_string(),
            width: None,
            height: None,
            revised_prompt: None,
        })
    }

    async fn renew_session(&self, _account: Account) -> Result<SessionInfo, MuseError> {
        Ok(SessionInfo {
            ok: true,
            cookies: HashMap::new(),
            cookie_expires: HashMap::new(),
            meta: [("driver".to_string(), Some("mock".to_string()))]
                .into_iter()
                .collect(),
        })
    }

    async fn quota(
        &self,
        _account: Account,
    ) -> Result<HashMap<String, serde_json::Value>, MuseError> {
        Ok([
            ("plan".to_string(), serde_json::json!("free")),
            ("messages_used".to_string(), serde_json::json!(0)),
            ("messages_limit".to_string(), serde_json::json!(100)),
            ("messages_remaining".to_string(), serde_json::json!(100)),
            ("media_used".to_string(), serde_json::json!(0)),
            ("media_limit".to_string(), serde_json::json!(20)),
            ("media_remaining".to_string(), serde_json::json!(20)),
            ("resets_at".to_string(), serde_json::Value::Null),
        ]
        .into_iter()
        .collect())
    }
}

/// Port `MockDriver.SolidPng` (.NET, dari Python `struct`/`zlib`):
/// PNG 8-bit RGB solid color, chunk IHDR/IDAT/IEND + CRC32.
/// Dipakai juga untuk test: header PNG harus valid.
pub fn solid_png(width: u32, height: u32, rgb: (u8, u8, u8)) -> Vec<u8> {
    use flate2::{write::ZlibEncoder, Compression};
    use std::io::Write;

    let mut out = Vec::new();
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 13, 10, 26, 10]);
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // bit depth 8, color type 2 (RGB)
    write_chunk(&mut out, "IHDR", &ihdr);
    let row_len = 1 + width as usize * 3;
    let mut raw = vec![0u8; row_len * height as usize];
    for y in 0..height as usize {
        let row = &mut raw[y * row_len..(y + 1) * row_len];
        row[0] = 0; // filter None
        for x in 0..width as usize {
            row[1 + x * 3] = rgb.0;
            row[2 + x * 3] = rgb.1;
            row[3 + x * 3] = rgb.2;
        }
    }
    let mut comp = Vec::new();
    {
        let mut enc = ZlibEncoder::new(&mut comp, Compression::default());
        enc.write_all(&raw).expect("zlib write");
        enc.finish().expect("zlib finish");
    }
    write_chunk(&mut out, "IDAT", &comp);
    write_chunk(&mut out, "IEND", &[]);
    out
}

fn write_chunk(out: &mut Vec<u8>, kind: &str, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let type_bytes = kind.as_bytes();
    out.extend_from_slice(type_bytes);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32_iter(type_bytes.iter().chain(data.iter())).to_be_bytes());
}

/// CRC-32 (IEEE) — port `MockDriver.Crc32` (.NET, tabel polinomial
/// `0xEDB88320`).
pub fn crc32(data: &[u8]) -> u32 {
    crc32_iter(data.iter())
}

fn crc32_iter<'a>(it: impl Iterator<Item = &'a u8>) -> u32 {
    let mut c: u32 = 0xFFFF_FFFF;
    for &b in it {
        c = CRC_TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

const CRC_TABLE: [u32; 256] = build_crc_table();

const fn build_crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut n = 0u32;
    while n < 256 {
        let mut c = n;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[n as usize] = c;
        n += 1;
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::InputImage;
    use futures_util::StreamExt;

    fn collect_stream(driver: &MockDriver, req: ChatRequest) -> Vec<ChatEvent> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let mut out = Vec::new();
            let mut s = driver.chat_stream(Account::new("acc_test"), req);
            while let Some(item) = s.next().await {
                out.push(item.expect("stream item"));
            }
            out
        })
    }

    #[test]
    fn mock_reply_exact_dotnet_line57() {
        // Persis Drivers.cs:57 — tanpa gambar.
        let req = ChatRequest::new("halo dunia", "muse-chat");
        assert_eq!(
            MockDriver::reply_for(&req),
            "[mock:muse-chat] You said: halo dunia"
        );
    }

    #[test]
    fn mock_reply_last_200_chars() {
        let long = "x".repeat(250);
        let req = ChatRequest::new(long.clone(), "muse-chat");
        let reply = MockDriver::reply_for(&req);
        assert_eq!(reply, format!("[mock:muse-chat] You said: {}", "x".repeat(200)));
        // Prompt pendek (< 200) tidak dipotong.
        let req = ChatRequest::new("abc", "gpt-4o");
        assert_eq!(MockDriver::reply_for(&req), "[mock:gpt-4o] You said: abc");
    }

    #[test]
    fn mock_reply_with_images_suffix() {
        let mut req = ChatRequest::new("lihat ini", "muse-chat");
        req.images.push(InputImage::png(vec![1, 2, 3]));
        req.images.push(InputImage::png(vec![4, 5, 6]));
        assert_eq!(
            MockDriver::reply_for(&req),
            "[mock:muse-chat] You said: lihat ini (with 2 image(s))"
        );
    }

    #[test]
    fn mock_stream_chunks_of_8_then_done() {
        let driver = MockDriver::with_delay(0);
        let req = ChatRequest::new("halo dunia", "muse-chat");
        let events = collect_stream(&driver, req);
        let mut text = String::new();
        let mut done = false;
        for (i, ev) in events.iter().enumerate() {
            match ev {
                ChatEvent::AssistantDelta(t) => {
                    assert!(t.chars().count() <= 8, "chunk {i} > 8 chars");
                    text += t;
                }
                ChatEvent::Done => {
                    done = true;
                    assert_eq!(i, events.len() - 1, "Done harus terakhir");
                }
            }
        }
        assert!(done);
        assert_eq!(text, "[mock:muse-chat] You said: halo dunia");
    }

    #[test]
    fn mock_stream_empty_prompt() {
        let driver = MockDriver::with_delay(0);
        let events = collect_stream(&driver, ChatRequest::new("", "muse-chat"));
        let mut text = String::new();
        for ev in &events {
            if let ChatEvent::AssistantDelta(t) = ev {
                text += t;
            }
        }
        assert_eq!(text, "[mock:muse-chat] You said: ");
        assert!(matches!(events.last(), Some(ChatEvent::Done)));
    }

    #[test]
    fn solid_png_valid_header_and_crc() {
        let png = solid_png(64, 64, (40, 120, 200));
        assert_eq!(&png[0..8], &[0x89, b'P', b'N', b'G', 13, 10, 26, 10]);
        // Chunk pertama harus IHDR, 13 byte.
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(u32::from_be_bytes(png[8..12].try_into().unwrap()), 13);
        // CRC-32 dikenal: crc32("123456789") == 0xCBF43926.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn mock_generate_image_returns_png() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let (results,) = rt.block_on(async {
            let d = MockDriver::with_delay(0);
            let r = d
                .generate_image(
                    Account::new("acc_test"),
                    ImageRequest::new("kucing", "muse-image"),
                )
                .await
                .unwrap();
            (r,)
        });
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].mime, "image/png");
        assert_eq!(&results[0].data[0..8], &[0x89, b'P', b'N', b'G', 13, 10, 26, 10]);
        assert_eq!(results[0].width, Some(64));
    }
}
