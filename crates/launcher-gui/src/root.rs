//! Root view: sidebar navigation plus the instances / browse / accounts /
//! settings pages, all rendered from a single entity for simplicity.

use gpui::{
    App, Context, Div, Entity, FocusHandle, Focusable, Render, SharedString, Stateful, Window,
    div, prelude::*, px, rgb,
};

use launcher_core::auth::store::AccountStore;
use launcher_core::auth::{self, Account, AccountKind};
use launcher_core::config::{Paths, Settings};
use launcher_core::instance::{self, Instance, LoaderConfig, LoaderKind};
use launcher_core::minecraft::manifest;
use launcher_core::modrinth;
use launcher_core::progress::{Progress, ProgressEvent};

use crate::bridge::run_bg;
use crate::text_input::TextInput;
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Instances,
    Browse,
    Accounts,
    Settings,
}

pub struct RootView {
    focus_handle: FocusHandle,
    tab: Tab,
    paths: Paths,
    settings: Settings,
    accounts: AccountStore,
    instances: Vec<Instance>,
    selected_instance: Option<String>,
    status: SharedString,
    busy: bool,

    // Instances tab
    name_input: Entity<TextInput>,
    version_input: Entity<TextInput>,
    with_fabric: bool,
    latest_release: Option<String>,

    // Browse (Modrinth) tab
    search_input: Entity<TextInput>,
    results: Vec<modrinth::SearchHit>,
    searching: bool,

    // Accounts tab
    offline_name_input: Entity<TextInput>,
    device_code: Option<auth::microsoft::DeviceCode>,
    logging_in: bool,

    // Settings tab
    client_id_input: Entity<TextInput>,
    java_path_input: Entity<TextInput>,
    memory_input: Entity<TextInput>,
}

impl RootView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let paths = Paths::from_env();
        let _ = std::fs::create_dir_all(&paths.root);
        let settings = Settings::load(&paths).unwrap_or_default();
        let accounts = AccountStore::load(&paths).unwrap_or_default();
        let instances = instance::list_instances(&paths).unwrap_or_default();
        let selected_instance = instances.first().map(|i| i.name.clone());

        let name_input = cx.new(|cx| TextInput::new(cx, "インスタンス名"));
        let version_input = cx.new(|cx| TextInput::new(cx, "バージョン (例: 1.21.4)"));
        let search_input = cx.new(|cx| TextInput::new(cx, "MODを検索 (例: sodium)"));
        let offline_name_input = cx.new(|cx| TextInput::new(cx, "プレイヤー名"));
        let client_id_input = cx.new(|cx| TextInput::new(cx, "Azure クライアントID"));
        let java_path_input = cx.new(|cx| TextInput::new(cx, "Javaパス (空欄で自動検出)"));
        let memory_input = cx.new(|cx| TextInput::new(cx, "メモリ (MiB)"));

        if let Some(id) = &settings.msa_client_id {
            let id = id.clone();
            client_id_input.update(cx, |input, cx| input.set_text(id, cx));
        }
        if let Some(path) = &settings.java_path {
            let path = path.clone();
            java_path_input.update(cx, |input, cx| input.set_text(path, cx));
        }
        let memory = settings.memory_mb.to_string();
        memory_input.update(cx, |input, cx| input.set_text(memory, cx));

        let mut this = Self {
            focus_handle: cx.focus_handle(),
            tab: Tab::Instances,
            paths,
            settings,
            accounts,
            instances,
            selected_instance,
            status: "準備完了".into(),
            busy: false,
            name_input,
            version_input,
            with_fabric: false,
            latest_release: None,
            search_input,
            results: Vec::new(),
            searching: false,
            offline_name_input,
            device_code: None,
            logging_in: false,
            client_id_input,
            java_path_input,
            memory_input,
        };
        this.fetch_latest_release(cx);
        this
    }

    fn set_status(&mut self, status: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.status = status.into();
        cx.notify();
    }

    /// A Progress sink whose events are forwarded into the status line.
    fn ui_progress(&self, cx: &mut Context<Self>) -> Progress {
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<ProgressEvent>();
        cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            let mut stage = String::new();
            while let Some(event) = rx.next().await {
                let text = match event {
                    ProgressEvent::Stage(name) => {
                        stage = name.clone();
                        name
                    }
                    ProgressEvent::Progress { done, total } => {
                        format!("{stage} ({done}/{total})")
                    }
                    ProgressEvent::Message(msg) => msg,
                };
                if this
                    .update(cx, |this, cx| this.set_status(text, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        Progress::new(std::sync::Arc::new(move |event| {
            let _ = tx.unbounded_send(event);
        }))
    }

    fn fetch_latest_release(&mut self, cx: &mut Context<Self>) {
        let paths = self.paths.clone();
        cx.spawn(async move |this, cx| {
            let result = run_bg(async move { manifest::fetch_manifest(&paths).await }).await;
            this.update(cx, |this, cx| {
                if let Ok(manifest) = result {
                    this.latest_release = Some(manifest.latest.release);
                    cx.notify();
                }
            })
            .ok()
        })
        .detach();
    }

    fn reload_instances(&mut self, cx: &mut Context<Self>) {
        self.instances = instance::list_instances(&self.paths).unwrap_or_default();
        if let Some(selected) = &self.selected_instance {
            if !self.instances.iter().any(|i| &i.name == selected) {
                self.selected_instance = self.instances.first().map(|i| i.name.clone());
            }
        } else {
            self.selected_instance = self.instances.first().map(|i| i.name.clone());
        }
        cx.notify();
    }

    // ----- Instances -----

    fn create_instance(&mut self, cx: &mut Context<Self>) {
        let name = self.name_input.read(cx).text().trim().to_string();
        let mut version = self.version_input.read(cx).text().trim().to_string();
        if version.is_empty() {
            version = self.latest_release.clone().unwrap_or_default();
        }
        if name.is_empty() || version.is_empty() {
            self.set_status("名前とバージョンを入力してください", cx);
            return;
        }
        let loader = self.with_fabric.then_some(LoaderConfig {
            kind: LoaderKind::Fabric,
            version: None,
        });
        match instance::create_instance(&self.paths, &name, &version, loader) {
            Ok(_) => {
                self.name_input.update(cx, |input, cx| input.set_text("", cx));
                self.selected_instance = Some(name.clone());
                self.set_status(format!("インスタンス '{name}' を作成しました"), cx);
                self.reload_instances(cx);
            }
            Err(e) => self.set_status(format!("作成に失敗: {e}"), cx),
        }
    }

    fn delete_instance(&mut self, name: String, cx: &mut Context<Self>) {
        match instance::delete_instance(&self.paths, &name) {
            Ok(()) => self.set_status(format!("'{name}' を削除しました"), cx),
            Err(e) => self.set_status(format!("削除に失敗: {e}"), cx),
        }
        self.reload_instances(cx);
    }

    fn launch_instance(&mut self, name: String, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let Some(account) = self.accounts.active_account().cloned() else {
            self.set_status("アカウントがありません。「アカウント」タブでログインしてください", cx);
            self.tab = Tab::Accounts;
            return;
        };
        let inst = match instance::load_instance(&self.paths, &name) {
            Ok(inst) => inst,
            Err(e) => {
                self.set_status(format!("読み込み失敗: {e}"), cx);
                return;
            }
        };
        self.busy = true;
        let paths = self.paths.clone();
        let settings = self.settings.clone();
        let client_id = self.settings.effective_client_id();
        let progress = self.ui_progress(cx);

        cx.spawn(async move |this, cx| {
            let result = run_bg(async move {
                let mut account = account;
                // Refresh expired Microsoft sessions before launching.
                if account.kind == AccountKind::Microsoft && !account.token_valid() {
                    let client_id = client_id.ok_or_else(|| {
                        launcher_core::Error::Auth(
                            "トークンの更新にはクライアントIDが必要です（設定タブ）".into(),
                        )
                    })?;
                    auth::microsoft::ensure_fresh(&mut account, &client_id).await?;
                }
                let prepared = instance::prepare(&paths, &settings, &inst, progress).await?;
                let child =
                    instance::launch_prepared(&paths, &settings, &inst, &prepared, &account)?;
                Ok::<_, launcher_core::Error>((account, child.id()))
            })
            .await;
            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok((account, pid)) => {
                        this.accounts.upsert(account);
                        let _ = this.accounts.save(&this.paths);
                        this.set_status(format!("Minecraft を起動しました (pid {pid})"), cx);
                    }
                    Err(e) => this.set_status(format!("起動に失敗: {e}"), cx),
                }
            })
            .ok()
        })
        .detach();
        self.set_status("起動準備中...", cx);
    }

    // ----- Browse -----

    fn run_search(&mut self, cx: &mut Context<Self>) {
        let query = self.search_input.read(cx).text().trim().to_string();
        if query.is_empty() || self.searching {
            return;
        }
        self.searching = true;
        let (game_version, loader) = self
            .selected_instance
            .as_ref()
            .and_then(|name| self.instances.iter().find(|i| &i.name == name))
            .map(|i| {
                (
                    Some(i.minecraft_version.clone()),
                    i.loader_name().map(str::to_string),
                )
            })
            .unwrap_or((None, None));
        self.set_status(format!("検索中: {query}"), cx);
        cx.spawn(async move |this, cx| {
            let result = run_bg(async move {
                modrinth::search(
                    &query,
                    modrinth::ProjectType::Mod,
                    game_version.as_deref(),
                    loader.as_deref(),
                    20,
                )
                .await
            })
            .await;
            this.update(cx, |this, cx| {
                this.searching = false;
                match result {
                    Ok(results) => {
                        this.set_status(
                            format!("{} 件見つかりました", results.hits.len()),
                            cx,
                        );
                        this.results = results.hits;
                    }
                    Err(e) => this.set_status(format!("検索に失敗: {e}"), cx),
                }
                cx.notify();
            })
            .ok()
        })
        .detach();
    }

    fn install_mod(&mut self, slug: String, cx: &mut Context<Self>) {
        let Some(inst) = self
            .selected_instance
            .as_ref()
            .and_then(|name| self.instances.iter().find(|i| &i.name == name))
            .cloned()
        else {
            self.set_status("先に「インスタンス」タブでインスタンスを選択してください", cx);
            return;
        };
        let paths = self.paths.clone();
        self.set_status(format!("{slug} をインストール中..."), cx);
        cx.spawn(async move |this, cx| {
            let inst_name = inst.name.clone();
            let result = run_bg(async move {
                let loader = inst.loader_name();
                let versions = modrinth::project_versions(
                    &slug,
                    Some(inst.minecraft_version.as_str()),
                    loader,
                )
                .await?;
                let version = versions.into_iter().next().ok_or_else(|| {
                    launcher_core::Error::Modrinth(format!(
                        "{slug} に Minecraft {} 対応版がありません",
                        inst.minecraft_version
                    ))
                })?;
                let mods_dir = inst.mods_dir(&paths);
                std::fs::create_dir_all(&mods_dir)?;
                let path = modrinth::download_version(&version, &mods_dir).await?;
                Ok::<_, launcher_core::Error>((version.name, path))
            })
            .await;
            this.update(cx, |this, cx| match result {
                Ok((name, _path)) => {
                    this.set_status(format!("{name} を {inst_name} に追加しました"), cx)
                }
                Err(e) => this.set_status(format!("インストールに失敗: {e}"), cx),
            })
            .ok()
        })
        .detach();
    }

    // ----- Accounts -----

    fn add_offline_account(&mut self, cx: &mut Context<Self>) {
        let name = self.offline_name_input.read(cx).text().trim().to_string();
        if name.is_empty() {
            self.set_status("プレイヤー名を入力してください", cx);
            return;
        }
        self.accounts.upsert(Account::offline(&name));
        let _ = self.accounts.save(&self.paths);
        self.offline_name_input
            .update(cx, |input, cx| input.set_text("", cx));
        self.set_status(format!("オフラインアカウント '{name}' を追加しました"), cx);
        cx.notify();
    }

    fn start_microsoft_login(&mut self, cx: &mut Context<Self>) {
        if self.logging_in {
            return;
        }
        let Some(client_id) = self.settings.effective_client_id() else {
            self.set_status(
                "Microsoftログインには「設定」タブでAzureクライアントIDの設定が必要です",
                cx,
            );
            self.tab = Tab::Settings;
            return;
        };
        self.logging_in = true;
        self.set_status("デバイスコードを取得中...", cx);
        cx.spawn(async move |this, cx| {
            let code = {
                let client_id = client_id.clone();
                run_bg(async move { auth::microsoft::request_device_code(&client_id).await }).await
            };
            let code = match code {
                Ok(code) => code,
                Err(e) => {
                    this.update(cx, |this, cx| {
                        this.logging_in = false;
                        this.set_status(format!("ログイン開始に失敗: {e}"), cx);
                    })
                    .ok();
                    return;
                }
            };
            this.update(cx, |this, cx| {
                this.device_code = Some(code.clone());
                this.set_status("ブラウザでコードを入力してください", cx);
            })
            .ok();
            let result = run_bg(async move {
                let ms = auth::microsoft::wait_for_device_login(&client_id, &code).await?;
                auth::microsoft::complete_login(&ms).await
            })
            .await;
            this.update(cx, |this, cx| {
                this.logging_in = false;
                this.device_code = None;
                match result {
                    Ok(account) => {
                        let name = account.username.clone();
                        this.accounts.upsert(account);
                        let _ = this.accounts.save(&this.paths);
                        this.set_status(format!("ログイン成功: {name}"), cx);
                    }
                    Err(e) => this.set_status(format!("ログインに失敗: {e}"), cx),
                }
            })
            .ok();
        })
        .detach();
    }

    fn select_account(&mut self, uuid: uuid::Uuid, cx: &mut Context<Self>) {
        self.accounts.active = Some(uuid);
        let _ = self.accounts.save(&self.paths);
        cx.notify();
    }

    fn remove_account(&mut self, uuid: uuid::Uuid, cx: &mut Context<Self>) {
        self.accounts.remove(uuid);
        let _ = self.accounts.save(&self.paths);
        cx.notify();
    }

    // ----- Settings -----

    fn save_settings(&mut self, cx: &mut Context<Self>) {
        let client_id = self.client_id_input.read(cx).text().trim().to_string();
        let java_path = self.java_path_input.read(cx).text().trim().to_string();
        let memory = self.memory_input.read(cx).text().trim().to_string();
        self.settings.msa_client_id = (!client_id.is_empty()).then_some(client_id);
        self.settings.java_path = (!java_path.is_empty()).then_some(java_path);
        if let Ok(mb) = memory.parse::<u32>() {
            self.settings.memory_mb = mb.clamp(512, 65536);
        }
        match self.settings.save(&self.paths) {
            Ok(()) => self.set_status("設定を保存しました", cx),
            Err(e) => self.set_status(format!("保存に失敗: {e}"), cx),
        }
    }

    // ----- Rendering helpers -----

    fn button(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        primary: bool,
    ) -> Stateful<Div> {
        let (bg, bg_hover, fg) = if primary {
            (theme::ACCENT, theme::ACCENT_HOVER, theme::ACCENT_TEXT)
        } else {
            (theme::BG_RAISED, theme::BG_HOVER, theme::TEXT)
        };
        div()
            .id(id)
            .px_3()
            .py_1p5()
            .rounded_md()
            .bg(rgb(bg))
            .text_color(rgb(fg))
            .text_size(px(13.))
            .cursor_pointer()
            .hover(move |style| style.bg(rgb(bg_hover)))
            .child(label.into())
    }

    fn sidebar_item(
        &self,
        id: &'static str,
        label: &'static str,
        tab: Tab,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let active = self.tab == tab;
        div()
            .id(id)
            .px_4()
            .py_2()
            .rounded_md()
            .cursor_pointer()
            .text_size(px(14.))
            .when(active, |el| {
                el.bg(rgb(theme::BG_RAISED)).text_color(rgb(theme::ACCENT))
            })
            .when(!active, |el| el.text_color(rgb(theme::TEXT_DIM)))
            .hover(|style| style.bg(rgb(theme::BG_RAISED)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.tab = tab;
                cx.notify();
            }))
            .child(label)
    }

    fn section_title(&self, text: &'static str) -> Div {
        div()
            .text_size(px(20.))
            .text_color(rgb(theme::TEXT))
            .mb_4()
            .child(text)
    }

    fn panel(&self) -> Div {
        div()
            .bg(rgb(theme::BG_RAISED))
            .rounded_lg()
            .p_4()
            .flex()
            .flex_col()
            .gap_3()
    }

    fn render_instances(&mut self, cx: &mut Context<Self>) -> Div {
        let latest = self
            .latest_release
            .clone()
            .map(|v| format!("最新リリース: {v}（バージョン欄を空にすると使用）"))
            .unwrap_or_else(|| "バージョン一覧を取得中...".to_string());

        let create_panel = self
            .panel()
            .child(
                div()
                    .text_size(px(15.))
                    .text_color(rgb(theme::TEXT))
                    .child("新しいインスタンス"),
            )
            .child(self.name_input.clone())
            .child(self.version_input.clone())
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(theme::TEXT_DIM))
                    .child(latest),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .id("fabric-toggle")
                            .px_3()
                            .py_1p5()
                            .rounded_md()
                            .cursor_pointer()
                            .text_size(px(13.))
                            .when(self.with_fabric, |el| {
                                el.bg(rgb(theme::ACCENT)).text_color(rgb(theme::ACCENT_TEXT))
                            })
                            .when(!self.with_fabric, |el| {
                                el.bg(rgb(theme::BG_HOVER)).text_color(rgb(theme::TEXT_DIM))
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.with_fabric = !this.with_fabric;
                                cx.notify();
                            }))
                            .child(if self.with_fabric {
                                "✓ Fabric を使う"
                            } else {
                                "Fabric を使う"
                            }),
                    )
                    .child(
                        self.button("create-instance", "作成", true)
                            .on_click(cx.listener(|this, _, _, cx| this.create_instance(cx))),
                    ),
            );

        let mut list = div().flex().flex_col().gap_2();
        if self.instances.is_empty() {
            list = list.child(
                div()
                    .text_color(rgb(theme::TEXT_DIM))
                    .text_size(px(13.))
                    .child("インスタンスがありません。上のフォームから作成してください。"),
            );
        }
        for (index, inst) in self.instances.clone().into_iter().enumerate() {
            let selected = self.selected_instance.as_deref() == Some(inst.name.as_str());
            let name = inst.name.clone();
            let name_for_select = name.clone();
            let name_for_launch = name.clone();
            let name_for_delete = name.clone();
            let loader = inst
                .loader_name()
                .map(|l| format!(" + {l}"))
                .unwrap_or_default();
            list = list.child(
                div()
                    .id(("instance", index))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .p_3()
                    .rounded_lg()
                    .bg(rgb(theme::BG_RAISED))
                    .border_1()
                    .border_color(rgb(if selected { theme::ACCENT } else { theme::BORDER }))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected_instance = Some(name_for_select.clone());
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .text_color(rgb(theme::TEXT))
                                    .child(name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(theme::TEXT_DIM))
                                    .child(format!("Minecraft {}{loader}", inst.minecraft_version)),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .child(
                                div()
                                    .id(("launch", index))
                                    .px_4()
                                    .py_1p5()
                                    .rounded_md()
                                    .bg(rgb(theme::ACCENT))
                                    .text_color(rgb(theme::ACCENT_TEXT))
                                    .text_size(px(13.))
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(theme::ACCENT_HOVER)))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.launch_instance(name_for_launch.clone(), cx);
                                    }))
                                    .child(if self.busy { "処理中..." } else { "▶ プレイ" }),
                            )
                            .child(
                                div()
                                    .id(("delete", index))
                                    .px_3()
                                    .py_1p5()
                                    .rounded_md()
                                    .bg(rgb(theme::BG_HOVER))
                                    .text_color(rgb(theme::DANGER))
                                    .text_size(px(13.))
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.delete_instance(name_for_delete.clone(), cx);
                                    }))
                                    .child("削除"),
                            ),
                    ),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(self.section_title("インスタンス"))
            .child(create_panel)
            .child(list)
    }

    fn render_browse(&mut self, cx: &mut Context<Self>) -> Div {
        let target = self
            .selected_instance
            .clone()
            .map(|name| format!("追加先: {name}"))
            .unwrap_or_else(|| "追加先インスタンスが未選択です".to_string());

        let search_row = div()
            .flex()
            .flex_row()
            .gap_3()
            .items_center()
            .child(div().flex_grow().child(self.search_input.clone()))
            .child(
                self.button("search", if self.searching { "検索中..." } else { "検索" }, true)
                    .on_click(cx.listener(|this, _, _, cx| this.run_search(cx))),
            );

        let mut list = div().id("results").flex().flex_col().gap_2().overflow_y_scroll();
        if self.results.is_empty() {
            list = list.child(
                div()
                    .text_color(rgb(theme::TEXT_DIM))
                    .text_size(px(13.))
                    .child("Modrinth で MOD を検索できます。"),
            );
        }
        for (index, hit) in self.results.clone().into_iter().enumerate() {
            let slug = hit.slug.clone();
            list = list.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .p_3()
                    .rounded_lg()
                    .bg(rgb(theme::BG_RAISED))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .max_w(px(560.))
                            .child(
                                div()
                                    .flex()
                                    .flex_row()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        div()
                                            .text_size(px(15.))
                                            .text_color(rgb(theme::TEXT))
                                            .child(hit.title.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.))
                                            .text_color(rgb(theme::TEXT_DIM))
                                            .child(format!(
                                                "by {} ・ {} DL",
                                                hit.author, hit.downloads
                                            )),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(theme::TEXT_DIM))
                                    .child(hit.description.clone()),
                            ),
                    )
                    .child(
                        div()
                            .id(("install", index))
                            .px_4()
                            .py_1p5()
                            .rounded_md()
                            .bg(rgb(theme::ACCENT))
                            .text_color(rgb(theme::ACCENT_TEXT))
                            .text_size(px(13.))
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(theme::ACCENT_HOVER)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.install_mod(slug.clone(), cx);
                            }))
                            .child("追加"),
                    ),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_4()
            .h_full()
            .child(self.section_title("MODを探す (Modrinth)"))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(theme::TEXT_DIM))
                    .child(target),
            )
            .child(search_row)
            .child(list)
    }

    fn render_accounts(&mut self, cx: &mut Context<Self>) -> Div {
        let ms_panel = self
            .panel()
            .child(
                div()
                    .text_size(px(15.))
                    .text_color(rgb(theme::TEXT))
                    .child("Microsoft アカウント"),
            )
            .child(
                self.button(
                    "ms-login",
                    if self.logging_in {
                        "サインイン待機中..."
                    } else {
                        "Microsoft でログイン"
                    },
                    true,
                )
                .on_click(cx.listener(|this, _, _, cx| this.start_microsoft_login(cx))),
            )
            .when_some(self.device_code.clone(), |el, code| {
                let uri = code.verification_uri.clone();
                el.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(rgb(theme::TEXT))
                                .child(format!("1. ブラウザで {} を開く", code.verification_uri)),
                        )
                        .child(
                            div()
                                .text_size(px(13.))
                                .text_color(rgb(theme::TEXT))
                                .child("2. 次のコードを入力:"),
                        )
                        .child(
                            div()
                                .text_size(px(24.))
                                .text_color(rgb(theme::ACCENT))
                                .child(code.user_code.clone()),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap_2()
                                .child(
                                    self.button("open-browser", "ブラウザで開く", false).on_click({
                                        move |_, _, _| {
                                            let _ = open::that(uri.clone());
                                        }
                                    }),
                                )
                                .child(
                                    self.button("copy-code", "コードをコピー", false).on_click({
                                        let user_code = code.user_code.clone();
                                        move |_, _, cx| {
                                            cx.write_to_clipboard(
                                                gpui::ClipboardItem::new_string(user_code.clone()),
                                            );
                                        }
                                    }),
                                ),
                        ),
                )
            });

        let offline_panel = self
            .panel()
            .child(
                div()
                    .text_size(px(15.))
                    .text_color(rgb(theme::TEXT))
                    .child("オフラインアカウント"),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .gap_3()
                    .child(div().flex_grow().child(self.offline_name_input.clone()))
                    .child(
                        self.button("add-offline", "追加", false)
                            .on_click(cx.listener(|this, _, _, cx| this.add_offline_account(cx))),
                    ),
            );

        let mut list = div().flex().flex_col().gap_2();
        for (index, account) in self.accounts.accounts.clone().into_iter().enumerate() {
            let active = self.accounts.active == Some(account.uuid);
            let uuid = account.uuid;
            let kind = match account.kind {
                AccountKind::Microsoft => "Microsoft",
                AccountKind::Offline => "オフライン",
            };
            list = list.child(
                div()
                    .id(("account", index))
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .p_3()
                    .rounded_lg()
                    .bg(rgb(theme::BG_RAISED))
                    .border_1()
                    .border_color(rgb(if active { theme::ACCENT } else { theme::BORDER }))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.select_account(uuid, cx)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .text_color(rgb(theme::TEXT))
                                    .child(format!(
                                        "{}{}",
                                        account.username,
                                        if active { "（使用中）" } else { "" }
                                    )),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(theme::TEXT_DIM))
                                    .child(format!("{kind} ・ {}", account.uuid)),
                            ),
                    )
                    .child(
                        div()
                            .id(("account-remove", index))
                            .px_3()
                            .py_1p5()
                            .rounded_md()
                            .bg(rgb(theme::BG_HOVER))
                            .text_color(rgb(theme::DANGER))
                            .text_size(px(13.))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove_account(uuid, cx);
                            }))
                            .child("削除"),
                    ),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(self.section_title("アカウント"))
            .child(ms_panel)
            .child(offline_panel)
            .child(list)
    }

    fn render_settings(&mut self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(self.section_title("設定"))
            .child(
                self.panel()
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(rgb(theme::TEXT))
                            .child("Microsoft ログイン用クライアントID"),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(theme::TEXT_DIM))
                            .child(
                                "Azure Portal でアプリを登録し、Minecraft API の利用申請を行った\
                                 クライアントIDを入力します（READMEを参照）。環境変数 MSA_CLIENT_ID でも設定できます。",
                            ),
                    )
                    .child(self.client_id_input.clone()),
            )
            .child(
                self.panel()
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(rgb(theme::TEXT))
                            .child("Java 実行ファイル"),
                    )
                    .child(self.java_path_input.clone()),
            )
            .child(
                self.panel()
                    .child(
                        div()
                            .text_size(px(14.))
                            .text_color(rgb(theme::TEXT))
                            .child("最大メモリ (MiB)"),
                    )
                    .child(self.memory_input.clone()),
            )
            .child(
                self.button("save-settings", "設定を保存", true)
                    .on_click(cx.listener(|this, _, _, cx| this.save_settings(cx))),
            )
    }
}

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = match self.tab {
            Tab::Instances => self.render_instances(cx),
            Tab::Browse => self.render_browse(cx),
            Tab::Accounts => self.render_accounts(cx),
            Tab::Settings => self.render_settings(cx),
        };

        let sidebar = div()
            .flex()
            .flex_col()
            .gap_1()
            .w(px(200.))
            .h_full()
            .p_3()
            .bg(rgb(theme::SIDEBAR))
            .child(
                div()
                    .px_4()
                    .py_3()
                    .text_size(px(17.))
                    .text_color(rgb(theme::ACCENT))
                    .child("⛏ Oxide Launcher"),
            )
            .child(self.sidebar_item("tab-instances", "インスタンス", Tab::Instances, cx))
            .child(self.sidebar_item("tab-browse", "MODを探す", Tab::Browse, cx))
            .child(self.sidebar_item("tab-accounts", "アカウント", Tab::Accounts, cx))
            .child(self.sidebar_item("tab-settings", "設定", Tab::Settings, cx));

        let status_bar = div()
            .flex()
            .flex_row()
            .items_center()
            .px_4()
            .py_2()
            .bg(rgb(theme::SIDEBAR))
            .border_t_1()
            .border_color(rgb(theme::BORDER))
            .text_size(px(12.))
            .text_color(rgb(theme::TEXT_DIM))
            .child(self.status.clone());

        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(theme::BG))
            .track_focus(&self.focus_handle)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_grow()
                    .min_h_0()
                    .child(sidebar)
                    .child(
                        div()
                            .id("content")
                            .flex_grow()
                            .h_full()
                            .p_6()
                            .overflow_y_scroll()
                            .child(content),
                    ),
            )
            .child(status_bar)
    }
}

impl Focusable for RootView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
