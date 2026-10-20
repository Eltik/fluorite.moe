mod catalog;
mod files;
mod i18n;
mod paths;
mod search;
mod text;
mod tree;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, Response};
use tower::ServiceExt;

use crate::api::search::SearchIndex;
use crate::api::{AppState, Config, router};

/// A throwaway output tree under the system temp dir, removed on drop.
pub struct Fixture {
    pub dir: PathBuf,
}

impl Fixture {
    pub fn new() -> Self {
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "fluorite-api-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let out = dir.join("output");
        let write = |rel: &str, bytes: &[u8]| {
            let p = out.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        };

        let png = |w: u32, h: u32| {
            let img = image::RgbaImage::from_fn(w, h, |x, y| {
                image::Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255])
            });
            let mut buf = std::io::Cursor::new(Vec::new());
            img.write_to(&mut buf, image::ImageFormat::Png).unwrap();
            buf.into_inner()
        };

        write("textures/ui/Pelica.png", &png(600, 300));
        write("textures/ui/pelica_icon_big.png", &png(8, 8));
        write("textures/ui/zeta.png", &png(4, 4));
        write("textures/ui/Alpha.png", &png(4, 4));
        write("textures/chars/pelica/body.png", &png(4, 4));
        write("textures/ui/.hidden.png", b"x");
        write("textures/ui/partial.png.tmp", b"x");
        write("textures/ui/blc_metadata.txt", b"x");
        write("textures/.index/stuff.png", b"x");
        std::fs::create_dir_all(out.join("textures/ui/sub")).unwrap();
        write("textures/ui/sub/one.png", b"x");

        let glb: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        write("meshes/chars/pelica.glb", &glb);
        write("decrypted/Table/TableCfg/Foo.json", br#"{"a":1}"#);
        write(
            "decrypted/Table/TableCfg/I18nTextTable_EN.json",
            br#"{"-1000162003504662904":"Naughty Little Knight\t","42":"Pelica","7":{"text":"Wrapped"},"8":5}"#,
        );
        write(
            "decrypted/Table/TableCfg/I18nTextTable_JP.json",
            r#"{"42":"ペリカ"}"#.as_bytes(),
        );
        write("decrypted/JsonData/Json/x.json", b"{}");
        write("decrypted/JsonData/Json/x.mpk", b"\0");
        write("decrypted/Lua/LuaScripts/main.lua", b"print('hi')");
        write(
            "update-log.jsonl",
            br#"{"at":"2026-10-20T08:35:02+00:00","version":"1.6.4","res_version":"r1"}"#,
        );

        std::fs::write(dir.join("secret.txt"), b"top secret").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.join("secret.txt"), out.join("textures/ui/escape.png"))
            .unwrap();

        Self { dir }
    }

    pub fn output(&self) -> PathBuf {
        self.dir.join("output")
    }

    pub fn config(&self) -> Config {
        let mut c = Config::for_output(self.output());
        c.cache_dir = self.dir.join("cache");
        c.state_file = self.dir.join("missing-state.json");
        c
    }

    /// Router over this fixture with the search index already built.
    pub fn app(&self) -> Router {
        let state = AppState::new(self.config());
        state.index.set(SearchIndex::build(&self.output()));
        router(state)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub fn exists(p: &Path) -> bool {
    p.exists()
}

pub async fn send(app: &Router, req: Request<Body>) -> Response<Body> {
    app.clone().oneshot(req).await.unwrap()
}

pub async fn get(app: &Router, uri: &str) -> Response<Body> {
    send(app, Request::get(uri).body(Body::empty()).unwrap()).await
}

pub async fn body_bytes(res: Response<Body>) -> Vec<u8> {
    axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap()
        .to_vec()
}

pub async fn body_json(res: Response<Body>) -> serde_json::Value {
    serde_json::from_slice(&body_bytes(res).await).unwrap()
}
