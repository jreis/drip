mod app;
mod config;
mod digest;
mod health;
mod import;
mod merge;
mod model;
mod oauth;
mod raindrop;
mod store;
mod sync;
mod ui;

use app::{App, InputMode};
use clap::{Parser, Subcommand};
use crossterm::ExecutableCommand;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use health::CheckJob;
use model::LinkHealth;
use ratatui::Terminal;
use ratatui::prelude::CrosstermBackend;
use std::io::{self, Write, stdout};
use std::path::PathBuf;
use std::time::Duration;

#[derive(Parser, Debug)]
#[command(
    name = "drip",
    about = "Local-first TUI for Raindrop.io bookmarks — search, filter, actually revisit them.",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Import a Raindrop.io CSV export into the local library
    Import {
        /// Path to the Raindrop CSV export
        path: PathBuf,
    },
    /// Authenticate with Raindrop (OAuth client id/secret, or test token)
    Auth {
        /// OAuth client id from https://app.raindrop.io/settings/integrations
        #[arg(long)]
        client_id: Option<String>,
        /// OAuth client secret
        #[arg(long)]
        client_secret: Option<String>,
        /// Redirect URI registered on the Raindrop app (default http://127.0.0.1:8787/callback)
        #[arg(long)]
        redirect_uri: Option<String>,
        /// Use a long-lived Test token instead of OAuth
        #[arg(long)]
        token: Option<String>,
        /// Print auth status only
        #[arg(long)]
        status: bool,
        /// Clear stored tokens (keeps client id/secret)
        #[arg(long)]
        logout: bool,
        /// Skip browser login; only save client id/secret
        #[arg(long)]
        save_only: bool,
    },
    /// Pull new/changed bookmarks from Raindrop API (preserves local status)
    Sync {
        /// Fetch the full library (ignore incremental watermark)
        #[arg(long)]
        full: bool,
        /// With --full, drop local items missing from Raindrop
        #[arg(long)]
        prune: bool,
        /// Don't write the library
        #[arg(long)]
        dry_run: bool,
    },
    /// Print library stats and data path
    Stats,
    /// Check link health (dead-link detection)
    Check {
        /// Only recheck links in this state (default: unknown). Use "all" to recheck everything.
        #[arg(long, default_value = "unknown")]
        only: String,
        /// Max number of links to check (0 = no limit)
        #[arg(long, default_value_t = 0)]
        limit: usize,
        /// Parallel workers
        #[arg(long, default_value_t = 12)]
        concurrency: usize,
        /// Skip saving results (dry run)
        #[arg(long)]
        dry_run: bool,
    },
    /// Launch the TUI (default when no subcommand is given)
    Tui,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Commands::Import { path }) => cmd_import(path),
        Some(Commands::Auth {
            client_id,
            client_secret,
            redirect_uri,
            token,
            status,
            logout,
            save_only,
        }) => cmd_auth(
            client_id,
            client_secret,
            redirect_uri,
            token,
            status,
            logout,
            save_only,
        ),
        Some(Commands::Sync {
            full,
            prune,
            dry_run,
        }) => cmd_sync(full, prune, dry_run),
        Some(Commands::Stats) => cmd_stats(),
        Some(Commands::Check {
            only,
            limit,
            concurrency,
            dry_run,
        }) => cmd_check(&only, limit, concurrency, dry_run),
        Some(Commands::Tui) | None => cmd_tui(),
    }
}

fn cmd_auth(
    client_id: Option<String>,
    client_secret: Option<String>,
    redirect_uri: Option<String>,
    token: Option<String>,
    status: bool,
    logout: bool,
    save_only: bool,
) -> anyhow::Result<()> {
    if status {
        for line in config::auth_status_lines()? {
            println!("{line}");
        }
        // Try live whoami if we can get a token.
        match config::require_access_token() {
            Ok(tok) => match raindrop::Client::new(tok).whoami() {
                Ok(who) => println!("whoami:    {who}"),
                Err(e) => println!("whoami:    error — {e}"),
            },
            Err(_) => println!("whoami:    (not authenticated)"),
        }
        return Ok(());
    }

    if logout {
        config::logout()?;
        println!("logged out (tokens cleared)");
        println!("config: {}", config::config_path()?.display());
        return Ok(());
    }

    if let Some(token) = token {
        config::set_test_token(token)?;
        let tok = config::require_access_token()?;
        match raindrop::Client::new(tok).whoami() {
            Ok(who) => println!("test token ok — authenticated as {who}"),
            Err(e) => println!("token saved, but whoami failed: {e}"),
        }
        println!("saved → {}", config::config_path()?.display());
        return Ok(());
    }

    // Prompt for missing credentials when needed.
    let mut client_id = client_id;
    let mut client_secret = client_secret;

    let existing = config::load()?;
    if client_id.is_none() {
        client_id = existing.client_id.clone();
    }
    if client_secret.is_none() {
        client_secret = existing.client_secret.clone();
    }

    if client_id.is_none() || client_secret.is_none() {
        println!("Raindrop OAuth setup");
        println!("  1. Open https://app.raindrop.io/settings/integrations");
        println!("  2. Create/open your app");
        println!(
            "  3. Set redirect URL to:  {}",
            redirect_uri
                .as_deref()
                .unwrap_or(config::DEFAULT_REDIRECT_URI)
        );
        println!();
        if client_id.is_none() {
            eprint!("Client ID: ");
            io::stderr().flush()?;
            let mut s = String::new();
            io::stdin().read_line(&mut s)?;
            client_id = Some(s.trim().to_string());
        }
        if client_secret.is_none() {
            eprint!("Client secret (hidden): ");
            io::stderr().flush()?;
            let s = rpassword::read_password()?;
            client_secret = Some(s.trim().to_string());
        }
    }

    let client_id = client_id.filter(|s| !s.is_empty()).ok_or_else(|| {
        anyhow::anyhow!("client id required — pass --client-id or run drip auth interactively")
    })?;
    let client_secret = client_secret.filter(|s| !s.is_empty()).ok_or_else(|| {
        anyhow::anyhow!(
            "client secret required — pass --client-secret or run drip auth interactively"
        )
    })?;

    let mut cfg = config::save_oauth_app(client_id, client_secret, redirect_uri)?;
    println!(
        "saved OAuth app credentials → {}",
        config::config_path()?.display()
    );
    println!("redirect URI: {}", cfg.redirect_uri());

    if save_only {
        println!("(--save-only) skipping browser login");
        return Ok(());
    }

    oauth::login_interactive(&mut cfg)?;
    Ok(())
}

fn cmd_sync(full: bool, prune: bool, dry_run: bool) -> anyhow::Result<()> {
    let report = sync::pull(sync::SyncOptions {
        full,
        prune,
        dry_run,
    })?;
    sync::print_report(&report);
    if dry_run {
        println!("dry-run: library not written");
    }
    Ok(())
}

fn cmd_import(path: PathBuf) -> anyhow::Result<()> {
    let existing = store::load_library()?;
    let lib = import::import_csv(&path, existing.as_ref())?;
    let n = lib.bookmarks.len();
    let stats = lib.stats();
    store::save_library(&lib)?;
    println!("imported {n} bookmarks from {}", path.display());
    println!(
        "  unread={} reading={} done={} skipped={} opened_once={}",
        stats.unread, stats.reading, stats.done, stats.skipped, stats.opened_once
    );
    println!(
        "  links: unknown={} alive={} dead={} error={} redirect={}",
        stats.link_unknown,
        stats.link_alive,
        stats.link_dead,
        stats.link_error,
        stats.link_redirect
    );
    println!("  library: {}", store::library_path()?.display());
    Ok(())
}

fn cmd_stats() -> anyhow::Result<()> {
    let path = store::library_path()?;
    match store::load_library()? {
        None => {
            println!("no library yet at {}", path.display());
            println!("run: drip import <raindrop.csv>");
        }
        Some(lib) => {
            let s = lib.stats();
            println!("library: {}", path.display());
            if let Some(src) = &lib.source_path {
                println!("source:  {src}");
            }
            if let Some(at) = lib.imported_at {
                println!("imported: {}", at.format("%Y-%m-%d %H:%M UTC"));
            }
            if let Some(at) = lib.last_synced_at {
                println!("synced:   {}", at.format("%Y-%m-%d %H:%M UTC"));
            }
            println!("total:   {}", s.total);
            println!("unread:  {}", s.unread);
            println!("reading: {}", s.reading);
            println!("done:    {}", s.done);
            println!("skipped: {}", s.skipped);
            println!("opened:  {}", s.opened_once);
            println!("--- link health ---");
            println!("unknown: {}", s.link_unknown);
            println!("alive:   {}", s.link_alive);
            println!("dead:    {}", s.link_dead);
            println!("error:   {}", s.link_error);
            println!("redir:   {}", s.link_redirect);
            println!("folders: {:?}", lib.folders());
        }
    }
    Ok(())
}

fn cmd_check(only: &str, limit: usize, concurrency: usize, dry_run: bool) -> anyhow::Result<()> {
    let mut lib = store::load_library()?.ok_or_else(|| {
        anyhow::anyhow!(
            "no library found — run: drip import <csv>\n  path: {}",
            store::library_path()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        )
    })?;

    let filter = only.to_ascii_lowercase();
    let mut jobs: Vec<CheckJob> = lib
        .bookmarks
        .iter()
        .filter(|b| match filter.as_str() {
            "all" => true,
            "unknown" | "unchecked" => b.link_health == LinkHealth::Unknown,
            "dead" => b.link_health == LinkHealth::Dead,
            "alive" => b.link_health == LinkHealth::Alive,
            "error" | "errors" => b.link_health == LinkHealth::Error,
            "redirect" | "redirects" => b.link_health == LinkHealth::Redirect,
            other => {
                eprintln!(
                    "unknown --only value: {other} (use all|unknown|dead|alive|error|redirect)"
                );
                std::process::exit(2);
            }
        })
        .map(|b| CheckJob {
            id: b.id.clone(),
            url: b.url.clone(),
        })
        .collect();

    if limit > 0 && jobs.len() > limit {
        jobs.truncate(limit);
    }

    if jobs.is_empty() {
        println!("nothing to check (filter={filter})");
        return Ok(());
    }

    println!(
        "checking {} links (concurrency={concurrency}, only={filter})…",
        jobs.len()
    );

    let results = health::check_blocking(jobs, concurrency, |done, total, r| {
        if done == total || done % 25 == 0 || matches!(r.health, LinkHealth::Dead) {
            eprint!(
                "\r  {done}/{total}  last={} {}                    ",
                r.health.as_str(),
                truncate_str(&r.id, 12)
            );
            let _ = io::Write::flush(&mut io::stderr());
        }
    });
    eprintln!();

    let mut alive = 0usize;
    let mut dead = 0usize;
    let mut errors = 0usize;
    let mut redirects = 0usize;
    for r in &results {
        match r.health {
            LinkHealth::Alive => alive += 1,
            LinkHealth::Dead => dead += 1,
            LinkHealth::Error => errors += 1,
            LinkHealth::Redirect => redirects += 1,
            LinkHealth::Unknown => {}
        }
        lib.apply_check_result(r);
    }

    println!("results: alive={alive} dead={dead} error={errors} redirect={redirects}");

    // Show a few dead samples.
    let dead_samples: Vec<_> = results
        .iter()
        .filter(|r| r.health == LinkHealth::Dead)
        .take(8)
        .collect();
    if !dead_samples.is_empty() {
        println!("sample dead links:");
        for r in dead_samples {
            if let Some(b) = lib.bookmarks.iter().find(|b| b.id == r.id) {
                println!(
                    "  ✗ {}  {}",
                    r.status_code
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "—".into()),
                    truncate_str(&b.url, 80)
                );
            }
        }
    }

    if dry_run {
        println!("dry-run: not saving");
    } else {
        store::save_library(&lib)?;
        println!("saved → {}", store::library_path()?.display());
    }
    Ok(())
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{t}…")
    }
}

fn cmd_tui() -> anyhow::Result<()> {
    // Optional sync-on-start before alternate screen.
    if config::load()?.sync_on_start {
        eprintln!("sync_on_start: pulling from Raindrop…");
        match sync::pull(sync::SyncOptions::default()) {
            Ok(r) => {
                eprintln!(
                    "  +{} ~{} total {}",
                    r.merge.added,
                    r.merge.updated,
                    r.library.bookmarks.len()
                );
            }
            Err(e) => eprintln!("  sync skipped: {e}"),
        }
    }

    let library = match store::load_library()? {
        Some(lib) => lib,
        None => {
            eprintln!("no library found. Import a CSV or sync from the API:");
            eprintln!("  drip import ~/Downloads/your-export.csv");
            eprintln!("  drip auth --client-id … --client-secret …");
            eprintln!("  drip sync --full");
            eprintln!("library path: {}", store::library_path()?.display());
            std::process::exit(1);
        }
    };

    let mut app = App::new(library);
    let mut terminal = setup_terminal()?;
    let result = run_loop(&mut terminal, &mut app);
    restore_terminal(&mut terminal)?;

    // Cancel any in-flight checks on exit.
    app.cancel_checks();

    if app.dirty {
        store::save_library(&app.library)?;
        eprintln!("saved library → {}", store::library_path()?.display());
    }

    result
}

fn setup_terminal() -> anyhow::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> anyhow::Result<()> {
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> anyhow::Result<()> {
    loop {
        app.poll_checks();
        terminal.draw(|f| ui::draw(f, app))?;

        if !event::poll(Duration::from_millis(80))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match app.mode {
            InputMode::Help => {
                app.mode = InputMode::Normal;
            }
            InputMode::Search => match key.code {
                KeyCode::Esc => {
                    app.query.clear();
                    app.mode = InputMode::Normal;
                    app.refilter();
                }
                KeyCode::Enter => {
                    app.mode = InputMode::Normal;
                    app.refilter();
                }
                KeyCode::Backspace => {
                    app.query.pop();
                    app.refilter();
                }
                KeyCode::Char(c) => {
                    app.query.push(c);
                    app.refilter();
                }
                _ => {}
            },
            InputMode::DomainBrowser => match key.code {
                KeyCode::Esc => {
                    if !app.domain_query.is_empty() {
                        app.domain_query.clear();
                        app.rebuild_domain_list();
                    } else {
                        app.mode = InputMode::Normal;
                        app.message.clear();
                    }
                }
                KeyCode::Enter => app.apply_domain_selection(),
                KeyCode::Down => app.domain_move(1, 20),
                KeyCode::Up => app.domain_move(-1, 20),
                KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.domain_move(1, 20);
                }
                KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.domain_move(-1, 20);
                }
                KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.domain_move(1, 20);
                }
                KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.domain_move(-1, 20);
                }
                KeyCode::Home => {
                    app.domain_selected = 0;
                    app.domain_offset = 0;
                }
                KeyCode::End => {
                    if !app.domain_list.is_empty() {
                        app.domain_selected = app.domain_list.len() - 1;
                    }
                }
                KeyCode::Backspace => {
                    app.domain_query.pop();
                    app.rebuild_domain_list();
                }
                // Type-to-filter: all printable chars (so "github" works)
                KeyCode::Char(c)
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::ALT) =>
                {
                    app.domain_query.push(c);
                    app.rebuild_domain_list();
                }
                _ => {}
            },
            InputMode::YearBrowser => match key.code {
                KeyCode::Esc => {
                    app.mode = InputMode::Normal;
                    app.message.clear();
                }
                KeyCode::Enter => app.apply_year_selection(),
                KeyCode::Char('0') => app.clear_year_filter(),
                KeyCode::Char('j') | KeyCode::Down => app.year_move(1),
                KeyCode::Char('k') | KeyCode::Up => app.year_move(-1),
                KeyCode::Char('g') => app.year_selected = 0,
                KeyCode::Char('G') if !app.year_list.is_empty() => {
                    app.year_selected = app.year_list.len() - 1;
                }
                _ => {}
            },
            InputMode::Digest => match key.code {
                KeyCode::Esc => app.dismiss_digest(),
                KeyCode::Enter => {
                    if let Err(e) = app.open_digest_selected() {
                        app.message = format!("open failed: {e}");
                    }
                }
                KeyCode::Char('j') | KeyCode::Down => app.digest_move(1),
                KeyCode::Char('k') | KeyCode::Up => app.digest_move(-1),
                KeyCode::Char('n') => app.digest_skip_selected(),
                KeyCode::Char('z') => app.reshuffle_digest(),
                KeyCode::Char('1') => {
                    app.digest_selected = 0;
                    let _ = app.open_digest_selected();
                }
                KeyCode::Char('2') => {
                    if app.digest_indices.len() > 1 {
                        app.digest_selected = 1;
                        let _ = app.open_digest_selected();
                    }
                }
                KeyCode::Char('3') => {
                    if app.digest_indices.len() > 2 {
                        app.digest_selected = 2;
                        let _ = app.open_digest_selected();
                    }
                }
                KeyCode::Char('q') => app.dismiss_digest(),
                _ => {}
            },
            InputMode::Normal => match key.code {
                KeyCode::Char('q') => {
                    app.should_quit = true;
                }
                KeyCode::Char('?') => app.mode = InputMode::Help,
                KeyCode::Char('/') => {
                    app.mode = InputMode::Search;
                    app.message.clear();
                }
                KeyCode::Esc => {
                    if !app.query.is_empty()
                        || app.domain_filter.is_some()
                        || app.year_filter.is_some()
                    {
                        if !app.query.is_empty() {
                            app.query.clear();
                            app.refilter();
                        } else {
                            app.clear_filters();
                        }
                    }
                }
                KeyCode::Char('j') | KeyCode::Down => app.move_sel(1, 0),
                KeyCode::Char('k') | KeyCode::Up => app.move_sel(-1, 0),
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.move_sel(20, 0);
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.move_sel(-20, 0);
                }
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.check_filtered(false);
                }
                KeyCode::Char('g') => {
                    app.selected = 0;
                    app.list_offset = 0;
                }
                KeyCode::Char('G') => {
                    if !app.filtered.is_empty() {
                        app.selected = app.filtered.len() - 1;
                    }
                }
                KeyCode::Enter => {
                    if let Err(e) = app.open_selected() {
                        app.message = format!("open failed: {e}");
                    }
                }
                KeyCode::Char('y') => {
                    if let Err(e) = app.copy_url() {
                        app.message = format!("copy failed: {e}");
                    }
                }
                KeyCode::Char(' ') => app.cycle_status(),
                KeyCode::Char('d') => app.mark_done(),
                KeyCode::Char('n') => app.mark_skipped(),
                KeyCode::Char('f') => app.toggle_folder_filter(),
                KeyCode::Char('s') => app.cycle_status_filter(),
                KeyCode::Char('l') => app.cycle_link_filter(),
                KeyCode::Char('D') => app.open_domain_browser(),
                KeyCode::Char('Y') => app.open_year_browser(),
                KeyCode::Char('.') => app.filter_domain_from_selected(),
                KeyCode::Char('0') => app.clear_filters(),
                KeyCode::Char('z') => app.show_digest(),
                KeyCode::Char('c') => app.check_selected(),
                KeyCode::Char('C') => app.check_filtered(true),
                KeyCode::Char('x') => {
                    if app.is_checking() {
                        app.cancel_checks();
                    }
                }
                KeyCode::Char('r') => {
                    app.random_pick();
                }
                KeyCode::Char('w') => {
                    store::save_library(&app.library)?;
                    app.dirty = false;
                    app.message = "saved".into();
                }
                KeyCode::Char('S') => {
                    // Leave alt screen briefly so sync progress is visible.
                    restore_terminal(terminal)?;
                    eprintln!("syncing from Raindrop…");
                    let sync_result = sync::pull(sync::SyncOptions::default());
                    *terminal = setup_terminal()?;
                    match sync_result {
                        Ok(r) => {
                            app.library = r.library;
                            app.dirty = false;
                            app.refilter();
                            app.message = format!(
                                "synced ({}) +{} ~{} total {}",
                                r.mode,
                                r.merge.added,
                                r.merge.updated,
                                app.library.bookmarks.len()
                            );
                        }
                        Err(e) => {
                            app.message = format!("sync failed: {e}");
                        }
                    }
                }
                _ => {}
            },
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}
