//! Public-network transport. Resolve every hop, reject non-public addresses, pin DNS answers.
use crate::error::{Error, Result};
use reqwest::{header::HeaderMap, Client, Method, Url};
use std::{
    net::{IpAddr, SocketAddr},
    time::Duration,
};
pub const MAX_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_PDF_BYTES: usize = 8 * 1024 * 1024;
// Below the relay's 100s deadline and the proxy/client's 120s/130s waits.
const ASR_TIMEOUT: Duration = Duration::from_secs(90);
// A complete editorial SSE response can exceed the generic web client's 60s.
// Keep text generation below the Pi RPC deadline (210s); no automatic replay.
pub(crate) const MODEL_TIMEOUT: Duration = Duration::from_secs(180);
pub fn blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192
                    && (b == 168 || (b == 0 && (c == 0 || c == 2)) || (b == 88 && c == 99)))
                || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                || (a == 203 && b == 0 && c == 113)
        }
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return blocked(IpAddr::V4(v4));
            }
            let s = ip.segments();
            !(0x2000..=0x3fff).contains(&s[0])
                || s[0] == 0x2002
                || (s[0] == 0x2001 && (s[1] == 0 || s[1] == 0xdb8 || s[1] < 0x200))
        }
    }
}
pub fn validate_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw).map_err(|_| Error::bad("网址无效"))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(Error::bad("仅允许无凭据的HTTP(S)网址"));
    }
    let host = url
        .host_str()
        .ok_or_else(|| Error::bad("网址缺少域名"))?
        .trim_matches(['[', ']'])
        .to_lowercase();
    if host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.ends_with(".internal")
        || host.parse::<IpAddr>().is_ok_and(blocked)
    {
        return Err(Error::bad("拒绝访问本地或私有网络"));
    }
    Ok(url)
}
pub async fn client_for(raw: &str) -> Result<Client> {
    client_for_timeout(raw, Duration::from_secs(60)).await
}
pub(crate) async fn model_client_for(raw: &str) -> Result<Client> {
    client_for_timeout(raw, MODEL_TIMEOUT).await
}
pub async fn asr_client_for(raw: &str) -> Result<Client> {
    client_for_timeout(raw, ASR_TIMEOUT).await
}
async fn client_for_timeout(raw: &str, timeout: Duration) -> Result<Client> {
    let url = validate_url(raw)?;
    let host = url.host_str().ok_or_else(|| Error::bad("域名无效"))?;
    let hostname = host.trim_matches(['[', ']']);
    let port = url.port_or_known_default().unwrap_or(443);
    let ips: Vec<SocketAddr> = tokio::time::timeout(
        Duration::from_secs(8),
        tokio::net::lookup_host((hostname, port)),
    )
    .await
    .map_err(|_| Error::upstream("timeout", "域名解析超时"))?
    .map_err(|_| Error::upstream("network", "无法解析来源域名"))?
    .collect();
    validate_answers(&ips)?;
    resolved_client(hostname, &ips, timeout)
}
fn resolved_client(hostname: &str, ips: &[SocketAddr], timeout: Duration) -> Result<Client> {
    Client::builder()
        .no_proxy()
        .resolve_to_addrs(hostname, ips)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(timeout)
        .user_agent("Memhot/0.1 (+https://memhot.angelkang.com)")
        .build()
        .map_err(|_| Error::internal())
}
fn validate_answers(ips: &[SocketAddr]) -> Result<()> {
    if ips.is_empty() || ips.iter().any(|a| blocked(a.ip())) {
        return Err(Error::bad("拒绝访问非公开网络地址"));
    }
    Ok(())
}
fn redirect_url(base: &Url, location: &str) -> Result<Url> {
    validate_url(
        base.join(location)
            .map_err(|_| Error::bad("重定向地址无效"))?
            .as_str(),
    )
}
pub fn transport_error(e: reqwest::Error) -> Error {
    if e.is_timeout() {
        Error::upstream("timeout", "外部服务请求超时")
    } else {
        Error::upstream("network", "外部服务连接失败")
    }
}
pub async fn bounded(mut response: reqwest::Response, max: usize) -> Result<Vec<u8>> {
    if response.content_length().is_some_and(|n| n > max as u64) {
        return Err(Error::upstream(
            "response_too_large",
            "来源响应超出大小限制",
        ));
    }
    let mut out = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
        if out.len() + chunk.len() > max {
            return Err(Error::upstream(
                "response_too_large",
                "来源响应超出大小限制",
            ));
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}
pub fn status_error(status: reqwest::StatusCode) -> Error {
    match status.as_u16() {
        401 | 403 => Error::upstream("provider_auth", "模型或搜索服务鉴权失败"),
        429 => Error::upstream("rate_limited", "外部服务暂时限流"),
        500..=599 => Error::upstream("upstream", "外部服务暂时不可用"),
        _ => Error::upstream("provider_rejected", "外部服务拒绝了请求"),
    }
}
pub async fn json_post(
    url: &str,
    headers: HeaderMap,
    body: serde_json::Value,
) -> Result<serde_json::Value> {
    let res = client_for(url)
        .await?
        .request(Method::POST, url)
        .headers(headers)
        .json(&body)
        .send()
        .await
        .map_err(transport_error)?;
    if !res.status().is_success() {
        return Err(status_error(res.status()));
    }
    serde_json::from_slice(&bounded(res, MAX_BYTES).await?)
        .map_err(|_| Error::upstream("invalid_response", "外部服务返回无效JSON"))
}
pub struct Fetched {
    pub url: String,
    pub text: String,
    pub content_type: String,
}
pub struct FetchedDocument {
    pub url: String,
    pub data: Vec<u8>,
    pub content_type: String,
    pub pdf: bool,
}
fn document_type(content_type: &str, data: &[u8]) -> Result<bool> {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let pdf = data.starts_with(b"%PDF-");
    if mime == "application/pdf" && !pdf {
        return Err(Error::bad("PDF来源未返回有效PDF文档"));
    }
    if !pdf && (mime.starts_with("image/") || mime.starts_with("video/")) {
        return Err(Error::bad("首版暂不提取此来源格式"));
    }
    Ok(pdf)
}
pub async fn fetch(raw: &str) -> Result<Fetched> {
    let fetched = fetch_document(raw).await?;
    if fetched.pdf {
        return Err(Error::bad("PDF正文须通过有界网页读取工具提取"));
    }
    let text = String::from_utf8(fetched.data).map_err(|_| Error::bad("来源不是有效UTF-8文本"))?;
    Ok(Fetched {
        url: fetched.url,
        text,
        content_type: fetched.content_type,
    })
}
/// One transport for HTML and PDF: every hop resolves, validates and pins DNS.
pub async fn fetch_document(raw: &str) -> Result<FetchedDocument> {
    let mut url = validate_url(raw)?;
    for hop in 0..=4 {
        let res = client_for(url.as_str())
            .await?
            .get(url.clone())
            .send()
            .await
            .map_err(transport_error)?;
        if res.status().is_redirection() {
            if hop == 4 {
                return Err(Error::bad("来源重定向过多"));
            }
            let next = res
                .headers()
                .get("location")
                .and_then(|h| h.to_str().ok())
                .ok_or_else(|| Error::bad("重定向地址无效"))?;
            url = redirect_url(&url, next)?;
            continue;
        }
        if !res.status().is_success() {
            return Err(status_error(res.status()));
        }
        let content_type = res
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let mime = content_type.split(';').next().unwrap_or("").trim();
        let possible_pdf = mime.eq_ignore_ascii_case("application/pdf")
            || mime.eq_ignore_ascii_case("application/octet-stream")
            || url.path().to_ascii_lowercase().ends_with(".pdf");
        let data = bounded(
            res,
            if possible_pdf {
                MAX_PDF_BYTES
            } else {
                MAX_BYTES
            },
        )
        .await?;
        let pdf = document_type(&content_type, &data)?;
        if !pdf && data.len() > MAX_BYTES {
            return Err(Error::upstream(
                "response_too_large",
                "来源响应超出大小限制",
            ));
        }
        return Ok(FetchedDocument {
            url: url.to_string(),
            data,
            content_type,
            pdf,
        });
    }
    Err(Error::internal())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pdf_type_requires_real_bytes_and_does_not_treat_a_login_page_as_pdf() {
        assert!(document_type("application/pdf; charset=binary", b"%PDF-1.7\n").unwrap());
        assert!(document_type("application/octet-stream", b"%PDF-1.4\n").unwrap());
        assert!(document_type("application/pdf", b"<html>login</html>").is_err());
        assert!(!document_type("text/html", b"<html>article</html>").unwrap());
        assert!(document_type("image/png", b"PNG").is_err());
    }
    #[tokio::test]
    async fn asr_transport_accepts_a_response_after_the_general_sixty_second_timeout() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let client = resolved_client("asr.example", &[address], ASR_TIMEOUT).unwrap();
        let request = tokio::spawn(async move {
            client
                .get(format!("http://asr.example:{}", address.port()))
                .send()
                .await
        });
        let (mut connection, _) = listener.accept().await.unwrap();
        let mut bytes = [0; 4096];
        connection.read(&mut bytes).await.unwrap();
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(61)).await;
        tokio::task::yield_now().await;
        assert!(
            !request.is_finished(),
            "a valid longer ASR call must remain in flight"
        );
        connection
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .await
            .unwrap();
        assert_eq!(request.await.unwrap().unwrap().status(), 200);
    }
    #[tokio::test]
    async fn asr_transport_is_still_bounded_before_the_relay_deadline() {
        use tokio::io::AsyncReadExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let client = resolved_client("asr.example", &[address], ASR_TIMEOUT).unwrap();
        let request = tokio::spawn(async move {
            client
                .get(format!("http://asr.example:{}", address.port()))
                .send()
                .await
        });
        let (mut connection, _) = listener.accept().await.unwrap();
        let mut bytes = [0; 4096];
        connection.read(&mut bytes).await.unwrap();
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(91)).await;
        assert!(request.await.unwrap().unwrap_err().is_timeout());
    }
    #[tokio::test]
    async fn model_stream_outlives_web_timeout_but_still_has_a_total_deadline() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for finish in [true, false] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let client = resolved_client("model.example", &[address], MODEL_TIMEOUT).unwrap();
            let request = tokio::spawn(async move {
                client
                    .get(format!("http://model.example:{}", address.port()))
                    .send()
                    .await
            });
            let (mut connection, _) = listener.accept().await.unwrap();
            let mut bytes = [0; 4096];
            connection.read(&mut bytes).await.unwrap();
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(121)).await;
            assert!(
                !request.is_finished(),
                "text calls must not inherit the 60s web limit"
            );
            if finish {
                connection
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                    )
                    .await
                    .unwrap();
                assert_eq!(request.await.unwrap().unwrap().status(), 200);
            } else {
                tokio::time::advance(Duration::from_secs(60)).await;
                assert!(request.await.unwrap().unwrap_err().is_timeout());
            }
            tokio::time::resume();
        }
    }
    #[test]
    fn rejects_ssrf_forms() {
        for url in [
            "file:///etc/passwd",
            "http://127.1",
            "http://[::1]",
            "http://[::ffff:127.0.0.1]",
            "http://169.254.169.254/latest",
            "https://localhost",
            "https://foo.internal",
            "https://user:secret@example.com",
        ] {
            assert!(validate_url(url).is_err(), "{url}");
        }
        for ip in [
            "10.1.2.3",
            "192.168.1.1",
            "172.31.4.3",
            "100.100.100.100",
            "198.18.0.1",
            "2001:db8::1",
            "64:ff9b::a00:1",
        ] {
            assert!(blocked(ip.parse().unwrap()));
        }
        assert!(!blocked("1.1.1.1".parse().unwrap()));
        assert!(!blocked("2606:4700:4700::1111".parse().unwrap()));
    }
    #[test]
    fn mixed_dns_answers_and_redirect_hops_are_revalidated() {
        assert!(validate_answers(&[]).is_err());
        assert!(validate_answers(&[
            "1.1.1.1:443".parse().unwrap(),
            "127.0.0.1:443".parse().unwrap()
        ])
        .is_err());
        assert!(validate_answers(&["1.1.1.1:443".parse().unwrap()]).is_ok());
        let base = Url::parse("https://example.com/article").unwrap();
        for location in [
            "http://169.254.169.254/",
            "//127.0.0.1/admin",
            "http://[::1]/",
            "file:///etc/passwd",
        ] {
            assert!(redirect_url(&base, location).is_err());
        }
        assert_eq!(
            redirect_url(&base, "/next").unwrap().as_str(),
            "https://example.com/next"
        );
    }
    #[tokio::test]
    async fn response_limit_checks_content_length_and_streamed_bytes() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for reply in ["HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n".to_owned(),"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n20\r\nabcdefghijklmnopqrstuvwxyzABCDEF\r\n0\r\n\r\n".to_owned()]{
            let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();
            tokio::spawn(async move{let(mut stream,_)=listener.accept().await.unwrap();let mut b=[0;1024];let _=stream.read(&mut b).await;let _=stream.write_all(reply.as_bytes()).await;});
            let response=Client::new().get(format!("http://{addr}/")).send().await.unwrap();
            assert_eq!(bounded(response,16).await.unwrap_err().code,"response_too_large");
        }
    }
}
