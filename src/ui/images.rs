//! Icon loader: background download + decode, disk cache, GPU textures uploaded once.

use crate::config::data_dir;
use egui::{ColorImage, Context, TextureHandle, TextureId, TextureOptions};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

enum State {
    Loading,
    Ready(TextureHandle),
    Failed,
}

pub struct Images {
    tex: HashMap<String, State>,
    req: Sender<String>,
    done: Receiver<(String, Option<ColorImage>)>,
}

fn cache_path(url: &str) -> PathBuf {
    let mut h = DefaultHasher::new();
    url.hash(&mut h);
    data_dir().join("img").join(format!("{:016x}.img", h.finish()))
}

fn load(url: &str) -> Option<ColorImage> {
    let path = cache_path(url);
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => {
            let b = crate::net::get_bytes(url).ok()?;
            let _ = std::fs::write(&path, &b);
            b
        }
    };
    let img = image::load_from_memory(&bytes).ok()?;
    // icons are drawn at <= 72 px; keep textures small
    let img = if img.width() > 128 { img.thumbnail(128, 128) } else { img };
    let rgba = img.to_rgba8();
    Some(ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
}

impl Images {
    pub fn new() -> Images {
        let _ = std::fs::create_dir_all(data_dir().join("img"));
        let (req, req_rx) = channel::<String>();
        let (done_tx, done) = channel();
        let req_rx = Arc::new(Mutex::new(req_rx));
        for i in 0..6 {
            let rx = req_rx.clone();
            let tx = done_tx.clone();
            std::thread::Builder::new()
                .name(format!("img{i}"))
                .spawn(move || loop {
                    let url = match rx.lock().unwrap().recv() {
                        Ok(u) => u,
                        Err(_) => return,
                    };
                    let img = load(&url);
                    if tx.send((url, img)).is_err() {
                        return;
                    }
                    crate::state::repaint();
                })
                .expect("spawn image loader");
        }
        Images { tex: HashMap::new(), req, done }
    }

    /// Texture for a URL, or None while it loads (a request is queued once).
    pub fn get(&mut self, ctx: &Context, url: &str) -> Option<TextureId> {
        if url.is_empty() {
            return None;
        }
        while let Ok((u, img)) = self.done.try_recv() {
            let state = match img {
                Some(img) => State::Ready(ctx.load_texture(&u, img, TextureOptions::LINEAR)),
                None => State::Failed,
            };
            self.tex.insert(u, state);
        }
        match self.tex.get(url) {
            Some(State::Ready(t)) => Some(t.id()),
            Some(_) => None,
            None => {
                self.tex.insert(url.to_string(), State::Loading);
                let _ = self.req.send(url.to_string());
                None
            }
        }
    }
}
