use crate::extraction::zip_range::{ZipEntry, fetch_range};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

// UNTESTABLE: from_split_archive, extract, extract_raw — require network

// Test ZipCentralDirectory accessors via a constructed instance
// These are tested indirectly since the struct fields are private,
// but we can test the parsing helpers that are pub(crate) or pub.

#[test]
fn zip_entry_fields() {
    let entry = ZipEntry {
        filename: "test.txt".to_string(),
        compression_method: 8,
        crc32: 0xDEADBEEF,
        compressed_size: 100,
        uncompressed_size: 200,
        local_header_offset: 0,
        disk_number: 0,
    };
    assert_eq!(entry.filename, "test.txt");
    assert_eq!(entry.compression_method, 8);
    assert_eq!(entry.compressed_size, 100);
    assert_eq!(entry.uncompressed_size, 200);
}

/// Serve one HTTP response per connection from `responses`, returning each request's head.
async fn serve(responses: Vec<Vec<u8>>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/part", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        let mut heads = Vec::new();
        for response in responses {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = socket.read(&mut buf).await.unwrap();
            heads.push(String::from_utf8_lossy(&buf[..n]).into_owned());
            socket.write_all(&response).await.unwrap();
            socket.shutdown().await.ok();
        }
        heads
    });
    (url, handle)
}

fn partial(body: &[u8], content_length: usize) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 206 Partial Content\r\nContent-Length: {content_length}\r\nConnection: close\r\n\r\n"
    )
    .into_bytes();
    out.extend_from_slice(body);
    out
}

#[tokio::test]
async fn fetch_range_resumes_after_a_dropped_body() {
    // The first response promises 10 bytes but the connection closes after 4.
    let (url, server) = serve(vec![partial(b"0123", 10), partial(b"456789", 6)]).await;
    let client = reqwest::Client::new();
    let body = fetch_range(&client, &url, 100, 109, None).await.unwrap();
    assert_eq!(body, b"0123456789");
    let heads = server.await.unwrap();
    assert!(
        heads[0]
            .to_ascii_lowercase()
            .contains("range: bytes=100-109")
    );
    assert!(
        heads[1]
            .to_ascii_lowercase()
            .contains("range: bytes=104-109")
    );
}

#[tokio::test]
async fn fetch_range_does_not_retry_client_errors() {
    let (url, server) = serve(vec![
        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
    ])
    .await;
    let client = reqwest::Client::new();
    let error = fetch_range(&client, &url, 0, 9, None).await.unwrap_err();
    assert!(error.to_string().contains("404"), "{error}");
    assert_eq!(server.await.unwrap().len(), 1);
}
