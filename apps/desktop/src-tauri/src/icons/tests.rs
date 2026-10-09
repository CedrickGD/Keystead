//! Website icon tests. Offline: HTML parsing, URL rules, address guards and
//! image processing work on fixtures; the fetcher runs against a local
//! plain-http server (`Allow::Local`, test builds only).

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv6Addr, TcpListener};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use image::codecs::bmp::BmpEncoder;
use image::codecs::gif::GifEncoder;
use image::codecs::ico::{IcoEncoder, IcoFrame};
use image::codecs::jpeg::JpegEncoder;
use image::codecs::webp::WebPEncoder;
use image::{ExtendedColorType, ImageEncoder as _, Rgba};
use keystead_core::model::{ItemType, LoginData, LoginUri, UriMatch, VaultItem};
use keystead_core::settings::Settings;
use keystead_core::KdfParams;

use super::*;
use crate::state::test_support::core_with_open_vault;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// An RGBA image `w`×`h`: opaque red on the left half, opaque blue right.
fn rgba(w: u32, h: u32) -> Vec<u8> {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for _y in 0..h {
        for x in 0..w {
            v.extend_from_slice(if x < w / 2 {
                &[220, 30, 30, 255]
            } else {
                &[30, 30, 220, 255]
            });
        }
    }
    v
}

fn png(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(&rgba(w, h), w, h, ExtendedColorType::Rgba8)
        .unwrap();
    out
}

fn ico(sizes: &[u32]) -> Vec<u8> {
    let raws: Vec<(u32, Vec<u8>)> = sizes.iter().map(|&s| (s, rgba(s, s))).collect();
    let frames: Vec<IcoFrame<'_>> = raws
        .iter()
        .map(|(s, raw)| IcoFrame::as_png(raw, *s, *s, ExtendedColorType::Rgba8).unwrap())
        .collect();
    let mut out = Vec::new();
    IcoEncoder::new(&mut out).encode_images(&frames).unwrap();
    out
}

fn jpeg(w: u32, h: u32) -> Vec<u8> {
    let rgb: Vec<u8> = rgba(w, h)
        .chunks(4)
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect();
    let mut out = Vec::new();
    JpegEncoder::new(&mut out)
        .write_image(&rgb, w, h, ExtendedColorType::Rgb8)
        .unwrap();
    out
}

fn gif(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut out);
        encoder
            .encode(&rgba(w, h), w, h, ExtendedColorType::Rgba8)
            .unwrap();
    }
    out
}

fn webp(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    WebPEncoder::new_lossless(&mut out)
        .encode(&rgba(w, h), w, h, ExtendedColorType::Rgba8)
        .unwrap();
    out
}

fn bmp(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::new();
    BmpEncoder::new(&mut out)
        .encode(&rgba(w, h), w, h, ExtendedColorType::Rgba8)
        .unwrap();
    out
}

fn decode(png: &[u8]) -> RgbaImage {
    image::load_from_memory_with_format(png, ImageFormat::Png)
        .unwrap()
        .into_rgba8()
}

fn page() -> Url {
    Url::parse("https://www.example.com/de/start?x=1").unwrap()
}

fn urls(candidates: &[Candidate]) -> Vec<String> {
    candidates
        .iter()
        .map(|c| match &c.source {
            Source::Url(u) => u.to_string(),
            Source::Png(b) => format!("data:{}", b.len()),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// HTML parsing
// ---------------------------------------------------------------------------

#[test]
fn finds_and_ranks_icon_links() {
    let html = r##"<!DOCTYPE html>
<html lang="de"><head>
  <meta charset="utf-8">
  <title>Example <link rel="icon" href="/in-title.png"></title>
  <!-- <link rel="icon" href="/commented.png"> -->
  <script>var s = '<link rel="icon" href="/in-script.png">';</script>
  <link rel="stylesheet" href="/site.css">
  <link rel="mask-icon" href="/mask.svg" color="#000">
  <link rel="icon" type="image/svg+xml" href="/icon.svg">
  <link rel="icon" href="/vector.SVG?v=2">
  <LINK REL="Shortcut Icon" HREF="/favicon.ico">
  <link rel=icon sizes=16x16 href=/small.png>
  <link rel="icon" type="image/png" sizes="32x32" href="icons/32.png">
  <link rel="icon" type="image/png" sizes="192x192" href="https://cdn.example.com/192.png?a=1&amp;b=2">
  <link rel="apple-touch-icon" href="/apple.png">
  <link rel="icon" sizes="96x96" href="/96.webp" type="image/webp">
  <link rel="icon" sizes="any" href="/any.png">
</head><body><link rel="icon" href="/in-body.png"></body></html>"##;
    let found = icon_candidates(html, &page());
    assert_eq!(
        urls(&found),
        vec![
            // ≥ 64: PNG/ICO before other types, then closest to 64 first.
            "https://www.example.com/apple.png",
            "https://cdn.example.com/192.png?a=1&b=2",
            "https://www.example.com/96.webp",
            // 32–63.
            "https://www.example.com/de/icons/32.png",
            // Unknown size: PNG before ICO.
            "https://www.example.com/any.png",
            "https://www.example.com/favicon.ico",
            // < 32 last.
            "https://www.example.com/small.png",
        ]
    );
    assert_eq!(found[0].rank(), (0, Kind::Png, 116));
}

#[test]
fn base_href_entities_and_data_urls() {
    let one_px = png(16, 16);
    let data = keystead_core::crypto::b64_encode(&one_px);
    let html = format!(
        r#"<head><base href="https://static.example.net/assets/">
<link rel="icon" href="fav.png?x=1&#38;y=2&amp;z=&#x33;">
<link rel="icon" href="data:image/png;base64,{data}" sizes="16x16">
<link rel="icon" href="data:image/gif;base64,R0lGODlhAQABAAAAACw=">
<link rel="icon" href="data:image/svg+xml,<svg/>">
<link rel="icon" href="javascript:alert(1)">
<link rel="icon" href="   ">
<link rel="icon" href="fav.png?x=1&y=2&z=3">"#
    );
    let found = icon_candidates(&html, &page());
    // Duplicates, GIF/SVG data, `javascript:` and empty links are left out.
    assert_eq!(found.len(), 2, "{:?}", urls(&found));
    assert_eq!(
        found[0].source,
        Source::Url(Url::parse("https://static.example.net/assets/fav.png?x=1&y=2&z=3").unwrap())
    );
    assert_eq!(found[1].source, Source::Png(one_px));

    // A non-web base is ignored.
    let found = icon_candidates(
        r#"<base href="file:///etc/"><link rel="icon" href="x.png">"#,
        &page(),
    );
    assert_eq!(urls(&found), vec!["https://www.example.com/de/x.png"]);
}

#[test]
fn malformed_html_does_not_panic() {
    for html in [
        "",
        "<",
        "<<<<",
        "</",
        "<!--",
        "<!-- never closed <link rel=icon href=/a.png>",
        "<link",
        "<link rel=\"icon\" href=\"/unterminated",
        "<link rel='icon' href='/a.png'",
        "<link =x rel=icon href=/b.png>",
        "<script><link rel=icon href=/c.png>",
        "a < b > c <link rel=icon href=/d.png>",
        "<link rel=icon href=\"/ä😀.png\">",
        "<link rel=icon href=\"&#xD800;&#99999999;&bogus;&\">",
        "\u{feff}<head><link rel=icon href=/e.png></head>",
    ] {
        let _ = icon_candidates(html, &page());
    }
    // Text "a < b" does not swallow the following tag.
    let found = icon_candidates("a < b > c <link rel=icon href=/d.png>", &page());
    assert_eq!(urls(&found), vec!["https://www.example.com/d.png"]);
    let found = icon_candidates("<link rel=icon href=\"/ä.png\">", &page());
    assert_eq!(urls(&found), vec!["https://www.example.com/%C3%A4.png"]);
}

#[test]
fn sizes_and_entities() {
    assert_eq!(parse_sizes("16x16 32x32"), Some(32));
    assert_eq!(parse_sizes("180X180"), Some(180));
    assert_eq!(parse_sizes("48x16"), Some(48));
    assert_eq!(parse_sizes("any"), None);
    assert_eq!(parse_sizes(""), None);
    assert_eq!(parse_sizes("axb 12x"), None);
    assert_eq!(decode_entities("a&amp;b&lt;&gt;&quot;&apos;"), "a&b<>\"'");
    assert_eq!(decode_entities("&#65;&#x42;&#X43;"), "ABC");
    assert_eq!(decode_entities("&nbsp;&;&#xZZ;& x"), "&nbsp;&;&#xZZ;& x");
    assert_eq!(decode_entities("ü&amp;ü"), "ü&ü");
}

// ---------------------------------------------------------------------------
// Address guards
// ---------------------------------------------------------------------------

#[test]
fn only_public_addresses() {
    for public in [
        "1.1.1.1",
        "8.8.8.8",
        "140.82.112.3",
        "2606:4700:4700::1111",
        "2a00:1450:4001:80b::200e",
        "::ffff:8.8.8.8",
        "64:ff9b::808:808",
        "2002:0808:0808::1",
    ] {
        assert!(is_public_ip(public.parse().unwrap()), "{public}");
    }
    for private in [
        "0.0.0.0",
        "0.1.2.3",
        "127.0.0.1",
        "127.255.0.9",
        "10.1.2.3",
        "172.16.0.1",
        "172.31.255.255",
        "192.168.178.1",
        "169.254.169.254",
        "100.64.0.1",
        "100.127.255.254",
        "192.0.0.8",
        "192.0.2.1",
        "198.18.0.1",
        "198.51.100.7",
        "203.0.113.9",
        "224.0.0.1",
        "239.255.255.250",
        "240.0.0.1",
        "255.255.255.255",
        "::",
        "::1",
        "::127.0.0.1",
        "::ffff:127.0.0.1",
        "::ffff:192.168.0.1",
        "fc00::1",
        "fd12:3456::1",
        "fe80::1",
        "fec0::1",
        "ff02::1",
        "2001:db8::1",
        "2001::1",
        "2001:10::1",
        "2001:20::1",
        "100::1",
        "64:ff9b::7f00:1",
        "64:ff9b:1::1",
        "2002:7f00:0001::1",
        "2002:c0a8:0001::1",
    ] {
        assert!(!is_public_ip(private.parse().unwrap()), "{private}");
    }
    assert!(is_public_ip(IpAddr::V6(Ipv6Addr::new(
        0x2001, 0x4860, 0, 0, 0, 0, 0, 0x8888
    ))));
}

#[test]
fn production_policy_allows_only_https_to_public_names() {
    let policy = Policy {
        allow: Allow::Public,
    };
    let ok = |u: &str| policy.url_allowed(&Url::parse(u).unwrap());
    assert!(ok("https://github.com/"));
    assert!(ok("https://github.githubassets.com/favicons/favicon.png"));
    assert!(ok("https://example.com:443/x"));
    assert!(!ok("http://github.com/"));
    assert!(!ok("https://github.com:8443/"));
    assert!(!ok("https://user:pw@github.com/"));
    assert!(!ok("https://127.0.0.1/"));
    assert!(!ok("https://[::1]/"));
    assert!(!ok("https://2130706433/"));
    assert!(!ok("https://0x7f.1/"));
    assert!(!ok("https://localhost/"));
    assert!(!ok("https://printer.local/"));
    assert!(!ok("https://intranet/"));
    assert!(!ok("ftp://example.com/"));
    assert!(!ok("file:///etc/passwd"));
    assert!(!ok("data:image/png;base64,AA=="));
    assert_eq!(
        policy.site_url("github.com", "favicon.ico").unwrap().as_str(),
        "https://github.com/favicon.ico"
    );
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tauri::async_runtime::block_on(f)
}

#[test]
fn the_resolver_hides_local_addresses() {
    // `localhost` resolves without a network (hosts file).
    let err = match block_on(PublicResolver.resolve("localhost".parse().unwrap())) {
        Ok(addrs) => panic!("resolved to {:?}", addrs.collect::<Vec<_>>()),
        Err(e) => e,
    };
    assert!(err.downcast_ref::<NonPublicAddress>().is_some(), "{err}");

    // The production client uses it for every connection: even a URL the
    // policy would refuse up front never reaches a local address.
    let fetcher = Fetcher::new("test").unwrap();
    let err = block_on(async { fetcher.client.get("https://localhost:9/").send().await })
        .unwrap_err();
    assert!(
        matches!(FetchError::from_reqwest(&err), FetchError::Refused(_)),
        "{err:?}"
    );
    // And the fetcher refuses such hosts before any request.
    for host in ["localhost", "127.0.0.1", "nas", "printer.local"] {
        assert!(matches!(
            block_on(fetcher.fetch(host)),
            Err(FetchError::Refused(_))
        ));
    }
}

// ---------------------------------------------------------------------------
// Images
// ---------------------------------------------------------------------------

#[test]
fn icons_are_fitted_into_64_by_64() {
    for (name, bytes) in [
        ("png", png(128, 128)),
        ("ico", ico(&[16, 32, 48])),
        ("jpeg", jpeg(180, 180)),
        ("gif", gif(32, 32)),
        ("webp", webp(96, 96)),
        ("small png", png(16, 16)),
    ] {
        let out = process_icon(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(keystead_core::icons::is_png(&out), "{name}");
        assert!(out.len() <= keystead_core::icons::ICON_MAX_PNG_BYTES, "{name}");
        let img = decode(&out);
        assert_eq!(img.dimensions(), (64, 64), "{name}");
        // Left red, right blue (the ICO's largest frame was used).
        let left = img.get_pixel(8, 32);
        let right = img.get_pixel(56, 32);
        assert!(left[0] > 150 && left[2] < 100, "{name}: {left:?}");
        assert!(right[2] > 150 && right[0] < 100, "{name}: {right:?}");
    }
}

#[test]
fn aspect_ratio_is_kept_with_transparent_padding() {
    let out = decode(&process_icon(&png(200, 100)).unwrap());
    assert_eq!(out.dimensions(), (64, 64));
    // 64×32 in the middle: rows 0..16 and 48..64 transparent.
    assert_eq!(out.get_pixel(32, 4), &Rgba([0, 0, 0, 0]));
    assert_eq!(out.get_pixel(32, 60), &Rgba([0, 0, 0, 0]));
    assert_eq!(out.get_pixel(32, 32)[3], 255);
    let tall = decode(&process_icon(&png(40, 80)).unwrap());
    assert_eq!(tall.get_pixel(2, 32), &Rgba([0, 0, 0, 0]));
    assert_eq!(tall.get_pixel(32, 32)[3], 255);
}

#[test]
fn unusable_images_are_refused() {
    let blank = {
        let mut out = Vec::new();
        PngEncoder::new(&mut out)
            .write_image(&vec![0u8; 32 * 32 * 4], 32, 32, ExtendedColorType::Rgba8)
            .unwrap();
        out
    };
    for (name, bytes) in [
        ("empty", Vec::new()),
        ("html", b"<!DOCTYPE html><html><body>Not found</body></html>".to_vec()),
        (
            "svg",
            br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1 1"/>"#.to_vec(),
        ),
        ("bmp", bmp(32, 32)),
        ("tiny", png(4, 4)),
        ("1 px", png(1, 1)),
        ("blank", blank),
        ("truncated", png(64, 64)[..40].to_vec()),
        ("garbage", vec![0x89, b'P', b'N', b'G', 1, 2, 3]),
    ] {
        assert!(process_icon(&bytes).is_err(), "{name}");
    }
    // A PNG header announcing a huge image is refused by the limits, not
    // decoded.
    let mut huge = png(8, 8);
    huge[16..20].copy_from_slice(&100_000u32.to_be_bytes());
    huge[20..24].copy_from_slice(&100_000u32.to_be_bytes());
    assert!(process_icon(&huge).is_err());
}

// ---------------------------------------------------------------------------
// Fetcher against a local server
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    delay: Duration,
}

fn reply(status: u16, body: impl Into<Vec<u8>>) -> Reply {
    Reply {
        status,
        headers: Vec::new(),
        body: body.into(),
        delay: Duration::ZERO,
    }
}

fn redirect(to: &str) -> Reply {
    Reply {
        headers: vec![("Location".into(), to.into())],
        ..reply(302, "")
    }
}

#[derive(Debug, Clone)]
struct Seen {
    path: String,
    headers: Vec<(String, String)>,
}

struct Server {
    origin: Url,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Server {
    fn paths(&self) -> Vec<String> {
        self.seen.lock().unwrap().iter().map(|s| s.path.clone()).collect()
    }
}

/// A plain-http server on 127.0.0.1; `route(path)` answers each request
/// (one thread per connection, `Connection: close`).
fn serve(route: impl Fn(&str) -> Reply + Send + Sync + 'static) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let route = Arc::new(route);
    let log = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let (route, log) = (Arc::clone(&route), Arc::clone(&log));
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    return;
                }
                let mut headers = Vec::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) <= 2 {
                        break;
                    }
                    if let Some((k, v)) = line.trim_end().split_once(':') {
                        headers.push((k.trim().to_ascii_lowercase(), v.trim().to_owned()));
                    }
                }
                let path = request_line.split(' ').nth(1).unwrap_or("/").to_owned();
                log.lock().unwrap().push(Seen {
                    path: path.clone(),
                    headers,
                });
                let r = route(&path);
                std::thread::sleep(r.delay);
                let mut head = format!(
                    "HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n",
                    r.status,
                    r.body.len()
                );
                for (k, v) in &r.headers {
                    head.push_str(&format!("{k}: {v}\r\n"));
                }
                head.push_str("\r\n");
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&r.body);
            });
        }
    });
    Server { origin, seen }
}

fn local_fetcher(server: &Server) -> Fetcher {
    Fetcher::build(
        Allow::Local {
            origin: server.origin.clone(),
        },
        "9.9.9-test",
        Duration::from_millis(1500),
        false,
    )
    .unwrap()
}

#[test]
fn loads_the_best_linked_icon_with_a_neutral_request() {
    let icon = png(180, 180);
    let server = serve(move |path| match path {
        "/a.example.com/" => reply(
            200,
            r#"<html><head>
                <link rel="icon" href="/a.example.com/broken.png" sizes="180x180">
                <link rel="apple-touch-icon" href="touch.png">
               </head></html>"#,
        ),
        "/a.example.com/broken.png" => reply(200, "<html>not an image</html>"),
        "/a.example.com/touch.png" => reply(200, icon.clone()),
        _ => reply(404, ""),
    });
    let fetcher = local_fetcher(&server);
    let out = block_on(fetcher.fetch("a.example.com")).unwrap();
    assert_eq!(decode(&out).dimensions(), (64, 64));
    assert_eq!(
        server.paths(),
        vec![
            "/a.example.com/",
            "/a.example.com/broken.png",
            "/a.example.com/touch.png"
        ]
    );
    for seen in server.seen.lock().unwrap().iter() {
        let header = |name: &str| {
            seen.headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        };
        assert_eq!(
            header("user-agent").as_deref(),
            Some("Keystead/9.9.9-test (icon fetcher)")
        );
        assert_eq!(header("cookie"), None);
        assert_eq!(header("referer"), None);
        assert_eq!(header("authorization"), None);
    }
}

#[test]
fn falls_back_to_favicon_ico() {
    let favicon = ico(&[16, 32]);
    let server = serve(move |path| match path {
        "/b.example.com/" => reply(403, "Forbidden"),
        "/b.example.com/favicon.ico" => reply(200, favicon.clone()),
        "/c.example.com/" => reply(200, "<head><title>no icons</title></head>"),
        "/c.example.com/favicon.ico" => reply(200, png(48, 48)),
        _ => reply(404, ""),
    });
    let fetcher = local_fetcher(&server);
    assert!(block_on(fetcher.fetch("b.example.com")).is_ok());
    assert!(block_on(fetcher.fetch("c.example.com")).is_ok());
    // Nothing anywhere: a failure, but the site answered (not "offline").
    let err = block_on(fetcher.fetch("d.example.com")).unwrap_err();
    assert!(matches!(err, FetchError::Failed(_)), "{err}");
}

#[test]
fn data_url_icons_need_no_request() {
    let data = keystead_core::crypto::b64_encode(&png(32, 32));
    let html = format!(r#"<link rel="icon" href="data:image/png;base64,{data}">"#);
    let server = serve(move |path| match path {
        "/e.example.com/" => reply(200, html.clone()),
        _ => reply(404, ""),
    });
    assert!(block_on(local_fetcher(&server).fetch("e.example.com")).is_ok());
    assert_eq!(server.paths(), vec!["/e.example.com/"]);
}

#[test]
fn redirects_are_limited_and_checked() {
    let icon = png(64, 64);
    let server = serve(move |path| match path {
        // Three redirects are fine …
        "/r3.example.com/" => redirect("/hop/1"),
        "/hop/1" => redirect("/hop/2"),
        "/hop/2" => redirect("/hop/3"),
        "/hop/3" => reply(200, r#"<link rel="icon" href="/icon.png">"#),
        "/icon.png" => reply(200, icon.clone()),
        // … four are not (also not for the favicon).
        "/r4.example.com/" | "/r4.example.com/favicon.ico" => redirect("/four/1"),
        "/four/1" => redirect("/four/2"),
        "/four/2" => redirect("/four/3"),
        "/four/3" => redirect("/four/4"),
        "/four/4" => reply(200, icon.clone()),
        // To another origin (in production: http, another port, a local
        // host): refused.
        "/away.example.com/" | "/away.example.com/favicon.ico" => {
            redirect("http://127.0.0.2:9/icon.png")
        }
        _ => reply(404, ""),
    });
    let fetcher = local_fetcher(&server);
    assert!(block_on(fetcher.fetch("r3.example.com")).is_ok());
    let err = block_on(fetcher.fetch("r4.example.com")).unwrap_err();
    assert!(matches!(err, FetchError::Refused(_)), "{err}");
    assert!(!server.paths().contains(&"/four/4".to_owned()));
    let err = block_on(fetcher.fetch("away.example.com")).unwrap_err();
    assert!(matches!(err, FetchError::Refused(_)), "{err}");
}

#[test]
fn large_bodies_and_slow_servers_are_cut_off() {
    let huge_icon = vec![0u8; MAX_BYTES + 1];
    let long_page = {
        let mut p = String::from(r#"<head><link rel="icon" href="/ok.png">"#);
        p.push_str(&"x".repeat(MAX_BYTES * 2));
        p
    };
    let icon = png(32, 32);
    let server = serve(move |path| match path {
        "/big.example.com/" => reply(200, r#"<link rel="icon" href="/huge.png">"#),
        "/huge.png" => reply(200, huge_icon.clone()),
        // A long page is read up to the limit; its head is enough.
        "/long.example.com/" => reply(200, long_page.clone()),
        "/ok.png" => reply(200, icon.clone()),
        "/slow.example.com/" | "/slow.example.com/favicon.ico" => Reply {
            delay: Duration::from_secs(4),
            ..reply(200, "")
        },
        _ => reply(404, ""),
    });
    let fetcher = local_fetcher(&server);
    assert!(block_on(fetcher.fetch("big.example.com")).is_err());
    assert!(block_on(fetcher.fetch("long.example.com")).is_ok());
    let started = Instant::now();
    let err = block_on(fetcher.fetch("slow.example.com")).unwrap_err();
    assert!(err.is_network(), "{err}");
    assert!(started.elapsed() < Duration::from_secs(4));
}

// ---------------------------------------------------------------------------
// Runs: storing into the right vault
// ---------------------------------------------------------------------------

fn login(name: &str, uri: &str) -> VaultItem {
    let mut item = VaultItem::new(ItemType::Login, name);
    item.login = Some(LoginData {
        username: "me".into(),
        password: "pw".into(),
        uris: vec![LoginUri {
            uri: uri.into(),
            match_type: UriMatch::Domain,
        }],
        ..Default::default()
    });
    item
}

fn icon_server(delay: Duration) -> Server {
    let icon = png(64, 64);
    serve(move |path| match path {
        "/a.example.com/favicon.ico" | "/c.example.com/favicon.ico" => Reply {
            delay,
            ..reply(200, icon.clone())
        },
        _ => Reply {
            delay,
            ..reply(404, "")
        },
    })
}

#[test]
fn a_run_stores_icons_in_one_go_and_tells_the_ui() {
    let dir = tempfile::tempdir().unwrap();
    let core = core_with_open_vault(dir.path(), Settings::default());
    for (name, uri) in [
        ("A", "https://www.a.example.com/login"),
        ("B", "https://b.example.com"),
        ("C", "c.example.com"),
        ("Router", "http://192.168.0.1"),
    ] {
        core.mutate(|v| v.save_item(login(name, uri))).unwrap();
    }
    let server = icon_server(Duration::ZERO);
    let fetcher = local_fetcher(&server);
    let rev = core.state().vault.as_ref().unwrap().revision();

    assert_eq!(run_once(&core, &fetcher), RunOutcome::Done { more_due: false });
    {
        let st = core.state();
        let vault = st.vault.as_ref().unwrap();
        assert!(vault.icon_for_host("a.example.com").is_some());
        assert!(vault.icon_for_host("c.example.com").is_some());
        assert_eq!(vault.icon_for_host("b.example.com"), None);
        assert!(vault.data().icons["b.example.com"].failed_at.is_some());
        assert!(!vault.data().icons.contains_key("192.168.0.1"));
        assert_eq!(vault.revision(), rev + 1, "one save for three sites");
        // The UI command and the bridge rows.
        let map = icon_map(vault);
        assert_eq!(map.len(), 2);
        assert!(map["a.example.com"].starts_with("data:image/png;base64,iVBOR"));
        let mut rows = vault.search("example");
        attach_icons(vault, &mut rows);
        let with_icon: Vec<&str> = rows
            .iter()
            .filter(|r| r.icon.is_some())
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(with_icon, vec!["A", "C"]);
    }
    assert!(core.emitted().iter().any(|(e, _)| e == EVENT_ICONS));
    assert_eq!(
        server
            .paths()
            .iter()
            .filter(|p| p.starts_with("/192.168"))
            .count(),
        0
    );
    // Nothing due any more.
    assert_eq!(run_once(&core, &fetcher), RunOutcome::Idle);

    // Off: no run.
    core.state().settings.website_icons = false;
    assert_eq!(run_once(&core, &fetcher), RunOutcome::Disabled);
}

#[test]
fn offline_runs_record_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let core = core_with_open_vault(dir.path(), Settings::default());
    core.mutate(|v| v.save_item(login("A", "https://a.example.com")))
        .unwrap();
    // A port nobody listens on: connection refused for every request.
    let closed = {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap()
    };
    let fetcher = Fetcher::build(
        Allow::Local { origin: closed },
        "test",
        Duration::from_millis(500),
        false,
    )
    .unwrap();
    assert_eq!(run_once(&core, &fetcher), RunOutcome::Offline);
    let st = core.state();
    assert!(st.vault.as_ref().unwrap().data().icons.is_empty());
}

#[test]
fn a_vault_switch_during_a_run_writes_nothing_into_the_new_vault() {
    let dir = tempfile::tempdir().unwrap();
    let core = core_with_open_vault(dir.path(), Settings::default());
    core.mutate(|v| v.save_item(login("A", "https://a.example.com")))
        .unwrap();
    let other = core
        .store()
        .create_vault_with_params("Other", "pw", KdfParams::insecure_for_tests())
        .unwrap();
    let other_id = other.id().to_owned();
    // The same site in the other vault: it must not get the icon fetched
    // for the first one.
    let server = icon_server(Duration::from_millis(700));
    let fetcher = local_fetcher(&server);
    let switcher = {
        let core = Arc::clone(&core);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            let mut other = other;
            other.save_item(login("A", "https://a.example.com")).unwrap();
            core.install_vault(other).unwrap();
        })
    };
    let outcome = run_once(&core, &fetcher);
    switcher.join().unwrap();
    assert!(
        matches!(outcome, RunOutcome::Cancelled | RunOutcome::Locked),
        "{outcome:?}"
    );
    let st = core.state();
    let vault = st.vault.as_ref().unwrap();
    assert_eq!(vault.id(), other_id);
    assert!(vault.data().icons.is_empty());
}

#[test]
fn locking_cancels_a_run() {
    let dir = tempfile::tempdir().unwrap();
    let core = core_with_open_vault(dir.path(), Settings::default());
    let id = core.state().vault.as_ref().unwrap().id().to_owned();
    core.mutate(|v| v.save_item(login("A", "https://a.example.com")))
        .unwrap();
    let server = icon_server(Duration::from_millis(700));
    let fetcher = local_fetcher(&server);
    let locker = {
        let core = Arc::clone(&core);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            core.lock(None);
            // Opened again at once: the old run still must not write.
            core.unlock(&id, "master").unwrap();
        })
    };
    let outcome = run_once(&core, &fetcher);
    locker.join().unwrap();
    assert_eq!(outcome, RunOutcome::Cancelled);
    assert!(core.state().vault.as_ref().unwrap().data().icons.is_empty());
}

/// Fetches the icons of a few popular sites through this machine's network
/// (the environment's proxy, if any – production uses none):
/// `cargo test -p keystead-desktop -- --ignored --nocapture real_sites`
#[test]
#[ignore = "needs the internet"]
fn real_sites() {
    let fetcher = Fetcher::build(Allow::Public, "test", REQUEST_TIMEOUT, true).unwrap();
    let out_dir = std::env::var_os("KEYSTEAD_ICON_OUT").map(std::path::PathBuf::from);
    for host in [
        "github.com",
        "wikipedia.org",
        "amazon.de",
        "paypal.com",
        "spiegel.de",
        "mozilla.org",
    ] {
        let started = Instant::now();
        match block_on(fetcher.fetch(host)) {
            Ok(png) => {
                let img = decode(&png);
                println!(
                    "{host}: ok, {} bytes, {:?}, {:?}",
                    png.len(),
                    img.dimensions(),
                    started.elapsed()
                );
                if let Some(dir) = &out_dir {
                    std::fs::write(dir.join(format!("{host}.png")), &png).unwrap();
                }
            }
            Err(e) => println!("{host}: {e} ({:?})", started.elapsed()),
        }
    }
}
