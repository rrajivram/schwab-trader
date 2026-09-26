//! Background work: a tokio runtime owned by the GUI. Each request runs as a
//! task and reports back over a std channel, waking the UI with a repaint.

use std::sync::mpsc::{channel, Receiver, Sender};

use eframe::egui;

use schwab::{accounts, auth};

pub enum Msg {
    /// Startup token check: Ok means a usable (possibly refreshed) token exists.
    TokenChecked(Result<(), String>),
    LoginBegun(Result<String, String>),
    LoginCompleted(Result<(), String>),
    AccountLoaded(Result<accounts::Account, String>),
}

pub struct Worker {
    rt: tokio::runtime::Runtime,
    tx: Sender<Msg>,
    pub rx: Receiver<Msg>,
    ctx: egui::Context,
}

impl Worker {
    pub fn new(ctx: egui::Context) -> Self {
        let (tx, rx) = channel();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("failed to start tokio runtime");
        Self { rt, tx, rx, ctx }
    }

    fn spawn<F>(&self, fut: F)
    where
        F: std::future::Future<Output = Msg> + Send + 'static,
    {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        self.rt.spawn(async move {
            let _ = tx.send(fut.await);
            ctx.request_repaint();
        });
    }

    pub fn check_token(&self) {
        self.spawn(async {
            Msg::TokenChecked(auth::get_valid_token().await.map(|_| ()).map_err(|e| e.to_string()))
        });
    }

    /// Not async, but kept on the worker so every result arrives the same way.
    pub fn begin_login(&self, app_key: String, app_secret: String) {
        self.spawn(async move {
            Msg::LoginBegun(auth::begin_login(&app_key, &app_secret).map_err(|e| e.to_string()))
        });
    }

    pub fn complete_login(&self, pasted_url: String) {
        self.spawn(async move {
            Msg::LoginCompleted(auth::complete_login(&pasted_url).await.map_err(|e| e.to_string()))
        });
    }

    /// The token only has access to a single account, so take the first one.
    /// The hash is re-fetched every time rather than cached (hashes go stale).
    pub fn load_account(&self) {
        self.spawn(async {
            let result = async {
                let hashes = accounts::list_account_numbers().await?;
                let first = hashes
                    .first()
                    .ok_or_else(|| anyhow::anyhow!("This token has no linked accounts"))?;
                accounts::get_account(first).await
            }
            .await;
            Msg::AccountLoaded(result.map_err(|e| e.to_string()))
        });
    }
}
