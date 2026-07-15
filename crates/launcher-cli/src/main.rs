//! `oxide` — command-line frontend for the launcher core.
//!
//! Useful for headless environments and for testing everything the GUI does.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use launcher_core::auth::{self, store::AccountStore, Account};
use launcher_core::config::{Paths, Settings};
use launcher_core::instance::{self, LoaderConfig, LoaderKind};
use launcher_core::minecraft::manifest;
use launcher_core::modrinth;
use launcher_core::progress::{Progress, ProgressEvent};
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "oxide", about = "Oxide Launcher CLI — Minecraft custom launcher")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Microsoftアカウントでログイン（デバイスコード方式）
    Login,
    /// 登録済みアカウントを表示
    Accounts,
    /// 使用するアカウントを選択
    UseAccount { name: String },
    /// 利用可能なMinecraftバージョンを表示
    Versions {
        /// スナップショット等も表示
        #[arg(long)]
        all: bool,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// インスタンスを作成
    Create {
        name: String,
        /// Minecraftバージョン（省略時は最新リリース）
        #[arg(long)]
        version: Option<String>,
        /// Fabricローダーを使う
        #[arg(long)]
        fabric: bool,
    },
    /// インスタンス一覧
    List,
    /// インスタンスを削除
    Delete { name: String },
    /// 必要ファイルをダウンロードのみ実行（起動しない）
    Prepare { name: String },
    /// インスタンスを起動
    Launch {
        name: String,
        /// 実際には起動せずコマンドラインを表示
        #[arg(long)]
        dry_run: bool,
    },
    /// ModrinthでMODを検索
    Search {
        query: String,
        #[arg(long)]
        version: Option<String>,
        #[arg(long)]
        loader: Option<String>,
        #[arg(long, default_value_t = 10)]
        limit: u32,
    },
    /// ModrinthのMODをインスタンスに追加
    Install {
        /// Modrinthのプロジェクトslugまたは ID（例: sodium）
        project: String,
        /// 追加先インスタンス
        instance: String,
    },
}

fn progress() -> Progress {
    Progress::new(Arc::new(|event| match event {
        ProgressEvent::Stage(name) => println!("==> {name}"),
        ProgressEvent::Progress { done, total } => {
            if done == total || done % 50 == 0 {
                println!("    {done}/{total}");
            }
        }
        ProgressEvent::Message(msg) => println!("    {msg}"),
    }))
}

fn client_id(settings: &Settings) -> Result<String> {
    settings.effective_client_id().context(
        "Microsoft ログインには Azure クライアントIDが必要です。\n\
         settings.json の msa_client_id か、環境変数 MSA_CLIENT_ID を設定してください。\n\
         （README の「Microsoftログインの設定」を参照）",
    )
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let paths = Paths::from_env();
    std::fs::create_dir_all(&paths.root)?;
    let settings = Settings::load(&paths)?;

    match cli.command {
        Command::Login => {
            let client_id = client_id(&settings)?;
            let code = auth::microsoft::request_device_code(&client_id).await?;
            println!("ブラウザで {} を開き、コードを入力してください:", code.verification_uri);
            println!("\n    {}\n", code.user_code);
            println!("サインインを待っています...");
            let ms = auth::microsoft::wait_for_device_login(&client_id, &code).await?;
            let account = auth::microsoft::complete_login(&ms).await?;
            println!("ログイン成功: {} ({})", account.username, account.uuid);
            let mut store = AccountStore::load(&paths)?;
            store.upsert(account);
            store.save(&paths)?;
        }
        Command::Accounts => {
            let store = AccountStore::load(&paths)?;
            if store.accounts.is_empty() {
                println!("アカウントが登録されていません。`oxide login` を実行してください。");
            }
            for account in &store.accounts {
                let active = if Some(account.uuid) == store.active { "*" } else { " " };
                println!("{active} {} [{:?}] {}", account.username, account.kind, account.uuid);
            }
        }
        Command::UseAccount { name } => {
            let mut store = AccountStore::load(&paths)?;
            let account = store
                .accounts
                .iter()
                .find(|a| a.username == name)
                .with_context(|| format!("アカウントが見つかりません: {name}"))?;
            store.active = Some(account.uuid);
            store.save(&paths)?;
            println!("アクティブアカウント: {name}");
        }
        Command::Versions { all, limit } => {
            let manifest = manifest::fetch_manifest(&paths).await?;
            println!("最新リリース: {}", manifest.latest.release);
            println!("最新スナップショット: {}", manifest.latest.snapshot);
            let versions = manifest
                .versions
                .iter()
                .filter(|v| all || v.kind == "release")
                .take(limit);
            for v in versions {
                println!("  {} ({})", v.id, v.kind);
            }
        }
        Command::Create { name, version, fabric } => {
            let version = match version {
                Some(v) => v,
                None => manifest::fetch_manifest(&paths).await?.latest.release,
            };
            let loader = fabric.then_some(LoaderConfig {
                kind: LoaderKind::Fabric,
                version: None,
            });
            let inst = instance::create_instance(&paths, &name, &version, loader)?;
            println!(
                "インスタンス '{}' を作成しました (Minecraft {}{})",
                inst.name,
                inst.minecraft_version,
                if fabric { " + Fabric" } else { "" }
            );
        }
        Command::List => {
            let instances = instance::list_instances(&paths)?;
            if instances.is_empty() {
                println!("インスタンスがありません。`oxide create <名前>` で作成してください。");
            }
            for inst in instances {
                let loader = inst.loader_name().map(|l| format!(" + {l}")).unwrap_or_default();
                println!("  {} — Minecraft {}{}", inst.name, inst.minecraft_version, loader);
            }
        }
        Command::Delete { name } => {
            instance::delete_instance(&paths, &name)?;
            println!("インスタンス '{name}' を削除しました");
        }
        Command::Prepare { name } => {
            let inst = instance::load_instance(&paths, &name)?;
            instance::prepare(&paths, &settings, &inst, progress()).await?;
            println!("準備完了: {name}");
        }
        Command::Launch { name, dry_run } => {
            let inst = instance::load_instance(&paths, &name)?;
            let mut store = AccountStore::load(&paths)?;
            // Dry runs only print the would-be command line and never start
            // the game, so they work without an account (placeholder values).
            let account = match store.active_account().cloned() {
                Some(account) => account,
                None if dry_run => Account::offline("Player"),
                None => bail!("アカウントがありません。`oxide login` を実行してください。"),
            };
            let mut account = account;
            if account.kind == auth::AccountKind::Microsoft && !account.token_valid() {
                println!("トークンを更新中...");
                let client_id = client_id(&settings)?;
                auth::microsoft::ensure_fresh(&mut account, &client_id).await?;
                store.upsert(account.clone());
                store.save(&paths)?;
            }
            let prepared = instance::prepare(&paths, &settings, &inst, progress()).await?;
            if dry_run {
                let game_dir = inst.game_dir(&paths);
                let ctx = launcher_core::minecraft::launch::LaunchContext {
                    paths: &paths,
                    version: &prepared.version,
                    jar_id: &prepared.jar_id,
                    game_dir: &game_dir,
                    classpath: &prepared.classpath,
                    account: &account,
                    java: &prepared.java.path,
                    memory_mb: inst.memory_mb.unwrap_or(settings.memory_mb),
                    extra_jvm_args: &settings.extra_jvm_args,
                };
                let cmd = launcher_core::minecraft::launch::build_command(&ctx)?;
                println!("--- 起動コマンド (dry run) ---");
                for (i, part) in cmd.iter().enumerate() {
                    // アクセストークンはログに出さない
                    let prev_is_token_flag = i > 0 && cmd[i - 1] == "--accessToken";
                    if prev_is_token_flag {
                        println!("  <access token>");
                    } else {
                        println!("  {part}");
                    }
                }
                return Ok(());
            }
            println!("起動します: {name} ({})", account.username);
            let mut child = instance::launch_prepared(&paths, &settings, &inst, &prepared, &account)?;
            let status = child.wait()?;
            println!("Minecraft が終了しました: {status}");
        }
        Command::Search { query, version, loader, limit } => {
            let results = modrinth::search(
                &query,
                modrinth::ProjectType::Mod,
                version.as_deref(),
                loader.as_deref(),
                limit,
            )
            .await?;
            println!("{} 件ヒット (表示 {})", results.total_hits, results.hits.len());
            for hit in results.hits {
                println!(
                    "  {} ({}) — {} DLs\n      {}",
                    hit.title, hit.slug, hit.downloads, hit.description
                );
            }
        }
        Command::Install { project, instance: inst_name } => {
            let inst = instance::load_instance(&paths, &inst_name)?;
            let loader = inst.loader_name();
            let versions = modrinth::project_versions(
                &project,
                Some(inst.minecraft_version.as_str()),
                loader,
            )
            .await?;
            let Some(version) = versions.first() else {
                bail!(
                    "{project} には Minecraft {}{} 対応のバージョンがありません",
                    inst.minecraft_version,
                    loader.map(|l| format!(" ({l})")).unwrap_or_default()
                );
            };
            let mods_dir = inst.mods_dir(&paths);
            std::fs::create_dir_all(&mods_dir)?;
            let path = modrinth::download_version(version, &mods_dir).await?;
            println!(
                "{} {} を {} に追加しました",
                version.name,
                version.version_number,
                path.display()
            );
        }
    }
    Ok(())
}
