//! Interactive setup wizard — captures parameters step by step and generates .env.

use anyhow::{Context, Result, bail};
use dialoguer::{theme::ColorfulTheme, Confirm, Input, Select};
use reqwest::Client;
use std::fs;
use std::path::PathBuf;

use crate::deck::EXPECTED_STACKS;
use crate::webdav::fetch_file_body;

/// List of available prompt variants with descriptions.
const PROMPT_VARIANTS: &[(&str, &str)] = &[
    (
        "agile-software-dev.md",
        "Agile & Software Development — sprint status, velocity, risk flags",
    ),
    (
        "home-improvement-structured.md",
        "Home Improvement & DIY (structured) — deadline-driven with urgency",
    ),
    (
        "home-improvement-unstructured.md",
        "Home Improvement & DIY (unstructured) — general household todo-list",
    ),
    ("meal-planning.md", "Meal Planning & Recipes — weekly menus, grocery lists"),
    (
        "personal-productivity.md",
        "Personal Productivity & Goal Tracking — habits, goals, weekly review",
    ),
    (
        "excursions-travel.md",
        "Excursions & Travel Itineraries — trip planning, booking, checklists",
    ),
];

// ─── Deck API helpers ───────────────────────────────────────────────

/// Validate that the NextCloud instance is reachable and the credentials work
/// by hitting the Deck API. Returns board list (id, title).
async fn validate_nextcloud(url: &str, user: &str, password: &str) -> Result<(String, Vec<(u64, String)>)> {
    let client = Client::new();
    let base = url.trim_end_matches('/');
    let mut checks = Vec::new();

    // 1. Deck API — validates auth + Deck app availability
    let decks_url = format!("{}/apps/deck/api/v1/boards", base);
    let resp = client
        .get(&decks_url)
        .basic_auth(user, Some(password))
        .send()
        .await
        .with_context(|| format!("Cannot reach Deck API at {}", decks_url))?;

    if resp.status() == 401 || resp.status() == 403 {
        bail!(
            "Authentication failed for {}.\n\
             Status: {}\n\
             Check your username and app-password.\n\
             Note: Use an 'App Password' from NextCloud user settings, \
             not your login password.\n\
             Create one here: {}/settings/user/security",
            base, resp.status(), base
        );
    }

    if !resp.status().is_success() {
        bail!(
            "Deck app is not available.\n\
             Status: {}\n\
             Make sure the Deck app is installed and enabled.",
            resp.status()
        );
    }
    checks.push("Deck API");

    let boards_json: serde_json::Value = resp
        .json()
        .await
        .context("Failed to parse boards JSON")?;

    let boards = boards_json
        .as_array()
        .unwrap_or(&serde_json::json!([]).as_array().unwrap().clone())
        .iter()
        .map(|b| {
            let id = b.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
            let title = b
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("Untitled")
                .to_string();
            (id, title)
        })
        .collect::<Vec<_>>();

    // 2. WebDAV — validates auth + files access
    let webdav_url = format!("{}/remote.php/dav/files/{}", base, user);
    let resp = client
        .get(&webdav_url)
        .basic_auth(user, Some(password))
        .send()
        .await
        .with_context(|| format!("Cannot reach WebDAV at {}", webdav_url))?;

    if resp.status() == 401 || resp.status() == 403 {
        bail!(
            "App-password lacks WebDAV/files access.\n\
             Status: {}\n\
             In NextCloud user settings, ensure the app-password \
             has 'Files' permission enabled.\n\
             Manage app passwords here: {}/settings/user/security",
            resp.status(), base
        );
    }

    if resp.status() == 404 {
        // WebDAV returned 404 — the user path doesn't exist under /files/
        // This can happen with certain NextCloud setups. Still valid auth.
        checks.push("WebDAV (read-only)");
    } else if resp.status().is_success() || resp.status().as_u16() == 207 {
        // 207 = multi-status (WebDAV PROPFIND response)
        checks.push("WebDAV");
    } else {
        checks.push("WebDAV");
    }

    Ok((format!("{} ({} OK)", base, checks.join(" + ")), boards))
}

/// Create a new Deck board.
/// POST /api/v1/boards  — returns { id, title }
async fn create_deck_board(
    client: &Client,
    base: &str,
    title: &str,
    user: &str,
    password: &str,
) -> Result<u64> {
    let url = format!("{}/api/v1/boards", base.trim_end_matches('/'));
    let body = serde_json::json!({
        "title": title,
        "description": "",
        "color": 6,
    });

    let resp = client
        .post(&url)
        .basic_auth(user, Some(password))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .with_context(|| format!("Failed to POST to {}", url))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        bail!(
            "Failed to create Deck board.\n\
             Status: {}\n\
             Response: {}",
            status,
            body_text
        );
    }

    let result: serde_json::Value = resp
        .json()
        .await
        .context("Failed to parse board creation response")?;

    result
        .get("id")
        .and_then(|v| v.as_u64())
        .context("Board creation response missing 'id'")
}

/// Add a stack to an existing board.
/// POST /api/v1/boards/{boardId}/stacks — returns { id, title }
async fn create_deck_stack(
    client: &Client,
    base: &str,
    board_id: u64,
    title: &str,
    user: &str,
    password: &str,
) -> Result<u64> {
    let url = format!("{}/api/v1/boards/{}/stacks", base.trim_end_matches('/'), board_id);
    let body = serde_json::json!({
        "title": title,
    });

    let resp = client
        .post(&url)
        .basic_auth(user, Some(password))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .with_context(|| format!("Failed to POST stack to {}", url))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        bail!(
            "Failed to create stack '{}'.\n\
             Status: {}\n\
             Response: {}",
            title,
            status,
            body_text
        );
    }

    let result: serde_json::Value = resp
        .json()
        .await
        .context("Failed to parse stack creation response")?;

    result
        .get("id")
        .and_then(|v| v.as_u64())
        .context("Stack creation response missing 'id'")
}

/// Check which expected stacks are missing from a board and offer to create them.
async fn configure_board(
    client: &Client,
    base: &str,
    board_id: u64,
    user: &str,
    password: &str,
    theme: &ColorfulTheme,
) -> Result<()> {
    // Fetch current stacks
    let url = format!("{}/apps/deck/api/v1/boards/{}", base.trim_end_matches('/'), board_id);
    let resp = client
        .get(&url)
        .basic_auth(user, Some(password))
        .send()
        .await
        .with_context(|| format!("Failed to fetch board {}", board_id))?;

    if !resp.status().is_success() {
        bail!("Failed to fetch board {} for stack check", board_id);
    }

    let board_value: serde_json::Value = resp
        .json()
        .await
        .context("Failed to parse board JSON")?;

    let found_stacks: Vec<String> = board_value
        .get("stacks")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.get("title").and_then(|s| s.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let missing: Vec<&str> = EXPECTED_STACKS
        .iter()
        .filter(|s| !found_stacks.contains(&s.to_string()))
        .copied()
        .collect();

    if missing.is_empty() {
        return Ok(());
    }

    println!(
        "\n⚠️  Board '{}' is missing {} required stack(s): {}",
        "selected board",
        missing.len(),
        missing.join(", ")
    );

    let create = Confirm::with_theme(theme)
        .with_prompt(&format!("Create missing stack(s) automatically? ({})", missing.join(", ")))
        .default(true)
        .interact()?;

    if !create {
        bail!(
            "Setup cannot proceed without all required stacks.\n\
             Please create the following stacks manually in Deck and try again:\n\
             {}",
            missing.join("\n- ")
        );
    }

    for stack_name in &missing {
        println!("   → Creating stack '{}'", stack_name);
        create_deck_stack(client, base, board_id, stack_name, user, password).await?;
    }

    println!("   ✓ All required stacks are now present");
    Ok(())
}

// ─── Setup wizard ───────────────────────────────────────────────────

/// Present the setup wizard and generate a .env file.
pub async fn run_setup() -> Result<()> {
    let theme = ColorfulTheme::default();

    println!();
    println!("╔══════════════════════════════════════════════════╗");
    println!("║  spark-project-status — Interactive Setup        ║");
    println!("╚══════════════════════════════════════════════════╝");
    println!();

    // ── Step 1: NextCloud URL ──────────────────────────────────────────
    let nc_url_raw: String = Input::with_theme(&theme)
        .with_prompt("1/8 NextCloud URL or hostname")
        .default(String::from("nextcloud.example.com"))
        .interact()?;

    // Normalize: prepend https:// if no scheme is given
    let nc_url = if nc_url_raw.contains("://") {
        nc_url_raw
    } else {
        format!("https://{}", nc_url_raw)
    };

    // ── Step 2: Credentials ────────────────────────────────────────────
    let nc_user: String = Input::with_theme(&theme)
        .with_prompt("2/8 NextCloud username")
        .default("your-username".to_string())
        .interact_text()?;

    let nc_password: String = dialoguer::Password::with_theme(&theme)
        .with_prompt("3/8 NextCloud app-password/token")
        .interact()?;

    println!("\n⏳ Validating credentials against {} …", nc_url.trim_end_matches('/'));

    // ── Step 3: Board selection ────────────────────────────────────────
    let client = Client::new();
    let base = nc_url.trim_end_matches('/');
    let (server_info, boards) = validate_nextcloud(&nc_url, &nc_user, &nc_password).await?;
    println!("   ✓ Connected to {}", server_info);
    println!();

    // Offer to create a new board if none exist
    if boards.is_empty() {
        println!("  No existing Deck boards found.");
        println!();

        let create_new = Confirm::with_theme(&theme)
            .with_prompt("Create a new Deck board?")
            .default(true)
            .interact()?;

        if !create_new {
            bail!(
                "No boards available.\n\
                 Please create a board in NextCloud Deck (with To-Do, Doing, Done, Cancelled stacks) \
                 and run setup again."
            );
        }

        let new_board_name: String = Input::with_theme(&theme)
            .with_prompt("New board name")
            .default("My Project".to_string())
            .interact_text()?;

        println!("\n   → Creating board '{}' …", new_board_name);
        let board_id = create_deck_board(&client, base, &new_board_name, &nc_user, &nc_password)
            .await?;
        println!("   ✓ Board created (ID: {})", board_id);

        // Configure stacks
        configure_board(&client, base, board_id, &nc_user, &nc_password, &theme).await?;

        let ncdeck_board_id = board_id;
        let board_title = new_board_name;

        // ── Step 5: Project details ──────────────────────────────────────
        let project_name: String = Input::with_theme(&theme)
            .with_prompt("5/8 Project name (used in report filenames)")
            .default(board_title.clone())
            .interact_text()?;

        let project_parent: String = Input::with_theme(&theme)
            .with_prompt("Project files parent path in NextCloud")
            .default("Personal".to_string())
            .interact_text()?;

        let spark_url: String = Input::with_theme(&theme)
            .with_prompt("6/8 Spark / LLM API base URL")
            .default("http://127.0.0.1:8214".to_string())
            .interact_text()?;

        let spark_api_key: String = Input::with_theme(&theme)
            .with_prompt("Spark API key (leave empty if no auth)")
            .default(String::from(""))
            .interact_text()?;

        let web_theme = Confirm::with_theme(&theme)
            .with_prompt("7/8 Prepend web-theme to report output? (YAML front-matter)")
            .default(true)
            .interact()?;

        let schedule_window: String = Input::with_theme(&theme)
            .with_prompt("8/8 Active scheduling window (e.g. 08:00-19:00, leave empty for always)")
            .default(String::new())
            .interact_text()?;

        // ── Step 9: Prompt/instructions selection ────────────────────────
        let (spark_instructions, was_uploaded) = prompt_selection_wizard(
            &client, base, &nc_user, &nc_password,
            &project_parent, &project_name, &theme,
        ).await?;

        // ── Generate .env ────────────────────────────────────────────────
        write_env_and_prompts(
            &nc_url, &nc_user, &nc_password, ncdeck_board_id,
            &spark_url, &spark_api_key,
            &project_name, &project_parent,
            &spark_instructions, was_uploaded, web_theme, &schedule_window,
        )?;
    } else {
        // ── Step 3 (cont): Select existing board ─────────────────────────
        let board_selection = Select::with_theme(&theme)
            .with_prompt("Select a Deck board")
            .items(&boards
                .iter()
                .map(|(id, title)| format!("{} (ID: {})", title, id))
                .collect::<Vec<_>>())
            .default(0)
            .interact()?;

        let ncdeck_board_id = boards[board_selection].0;
        let board_title = boards[board_selection].1.clone();

        println!("   Selected: {}", board_title);

        // ── Step 4: Board configurator ───────────────────────────────────
        configure_board(&client, base, ncdeck_board_id, &nc_user, &nc_password, &theme).await?;
        println!();

        // ── Step 5: Project details ──────────────────────────────────────
        let project_name: String = Input::with_theme(&theme)
            .with_prompt("5/8 Project name (used in report filenames)")
            .default(board_title.clone())
            .interact_text()?;

        let project_parent: String = Input::with_theme(&theme)
            .with_prompt("Project files parent path in NextCloud")
            .default("Personal".to_string())
            .interact_text()?;

        let spark_url: String = Input::with_theme(&theme)
            .with_prompt("6/8 Spark / LLM API base URL")
            .default("http://127.0.0.1:8214".to_string())
            .interact_text()?;

        let spark_api_key: String = Input::with_theme(&theme)
            .with_prompt("Spark API key (leave empty if no auth)")
            .default(String::from(""))
            .interact_text()?;

        let web_theme = Confirm::with_theme(&theme)
            .with_prompt("7/8 Prepend web-theme to report output? (YAML front-matter)")
            .default(true)
            .interact()?;

        let schedule_window: String = Input::with_theme(&theme)
            .with_prompt("8/8 Active scheduling window (e.g. 08:00-19:00, leave empty for always)")
            .default(String::new())
            .interact_text()?;

        // ── Step 9: Prompt/instructions selection ────────────────────────
        let (spark_instructions, was_uploaded) = prompt_selection_wizard(
            &client, base, &nc_user, &nc_password,
            &project_parent, &project_name, &theme,
        ).await?;

        // ── Generate .env ────────────────────────────────────────────────
        write_env_and_prompts(
            &nc_url, &nc_user, &nc_password, ncdeck_board_id,
            &spark_url, &spark_api_key,
            &project_name, &project_parent,
            &spark_instructions, was_uploaded, web_theme, &schedule_window,
        )?;
    }

    println!();
    println!("Setup complete! Run:");
    println!();
    println!("   spark-project-status --dry-run");
    println!();

    Ok(())
}

/// Generate the SCHEDULE_WINDOW env line.
fn schedule_window_line(window: &str) -> String {
    if window.trim().is_empty() {
        String::new()
    } else {
        format!("SCHEDULE_WINDOW=\"{}\"\n", window.trim())
    }
}

/// Write the `.env` file and copy the selected Spark-Instructions.md.
fn write_env_and_prompts(
    nc_url: &str, nc_user: &str, nc_password: &str, ncdeck_board_id: u64,
    spark_url: &str, spark_api_key: &str,
    project_name: &str, project_parent: &str,
    spark_instructions: &str,
    was_uploaded: bool,
    web_theme: bool,
    schedule_window: &str,
) -> Result<()> {
    let env_content = format!(
        "# Auto-generated by spark-project-status setup\n\
         # See sample.env for documentation of all parameters.\n\n\
         NC_URL=\"{nc_url}\"\n\
         NC_APP_USER=\"{nc_user}\"\n\
         NC_APP_PWD=\"{nc_password}\"\n\
         NCDECK_BOARD_ID={ncdeck_board_id}\n\
         SPARK_URL=\"{spark_url}\"\n\
         SPARK_API_KEY=\"{spark_api_key}\"\n\
         PROJECT_NAME=\"{project_name}\"\n\
         PROJECT_FILES_PARENT_PATH=\"{project_parent}\"\n\
         WEB_THEME={web_theme}\n\
         {}\n", schedule_window_line(schedule_window),
    );

    let env_path = PathBuf::from(".env");
    fs::write(&env_path, &env_content)
        .with_context(|| format!("Failed to write {}", env_path.display()))?;
    println!("   ✓ Created {}", env_path.display());

    // Write Spark-Instructions.md locally
    let spark_instr = PathBuf::from("Spark-Instructions.md");
    if was_uploaded {
        // Instructions were uploaded from a template — write local copy
        fs::write(&spark_instr, spark_instructions)
            .with_context(|| format!("Failed to write {}", spark_instr.display()))?;
        println!("   ✓ Saved {} locally", spark_instr.display());
    } else {
        // Instructions were fetched from remote — write local copy
        fs::write(&spark_instr, spark_instructions)
            .with_context(|| format!("Failed to write {}", spark_instr.display()))?;
        println!("   ✓ Saved remote instructions locally");
    }

    // Copy web-theme if missing
    let web_theme = PathBuf::from("web-theme.txt");
    let default_theme = PathBuf::from("web-theme-default.txt");
    if !web_theme.exists() && default_theme.exists() {
        fs::copy(&default_theme, &web_theme)?;
        println!("   ✓ Copied web-theme-default.txt → web-theme.txt");
    }

    Ok(())
}

/// Check if we should enter interactive mode.
/// Returns true if no env file exists and no CLI overrides are given.
pub fn should_interact() -> bool {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h" || a == "--version" || a == "-V" || a == "--show-config") {
        return false;
    }

    if std::env::args().any(|a| a == "--config" || a == "-c") {
        return false;
    }

    let config_flags = [
        "--nc-url", "--nc-user", "--nc-password", "--board-id",
        "--spark-url", "--api-key", "--project-name", "--project-parent",
        "--web-theme", "--schedule", "--print", "--save-only", "--filename", "--copy-dav",
    ];
    if config_flags.iter().any(|f| std::env::args().any(|a| a == *f)) {
        return false;
    }

    !PathBuf::from(".env").exists()
}

/// Prompt the user to choose a prompt strategy.
///
/// Checks whether Spark-Instructions.md already exists remotely. If it does,
/// offers the user the choice between:
/// 1. Keep and use the existing remote instructions
/// 2. Select from built-in prompt templates
///
/// Returns the Spark-Instructions.md content and whether it was uploaded.
pub async fn prompt_selection_wizard(
    client: &Client,
    base: &str,
    user: &str,
    password: &str,
    project_parent: &str,
    project_name: &str,
    theme: &ColorfulTheme,
) -> Result<(String, bool)> {
    // Determine remote path for Spark-Instructions.md
    let remote_instr_path = format!("remote.php/dav/files/{}/{}/{}/Spark-Instructions.md",
        user, project_parent, project_name);

    // Try to fetch existing remote instructions
    let existing_remote = fetch_file_body(&base, &remote_instr_path, user, password)
        .await
        .ok(); // Silently ignore — file may not exist yet

    let mut options = Vec::new();
    let mut source_map: Vec<PromptSource> = Vec::new();

    if let Some(ref content) = existing_remote {
        options.push(format!("Keep existing remote instructions ({} chars)", content.len()));
        source_map.push(PromptSource::Remote {
            path: remote_instr_path.clone(),
            content: content.clone(),
        });
    }

    for (_, desc) in PROMPT_VARIANTS.iter() {
        options.push((*desc).to_string());
        source_map.push(PromptSource::Template(desc.to_string()));
    }

    let selection = Select::with_theme(theme)
        .with_prompt("Choose prompt source")
        .items(&options)
        .default(if existing_remote.is_some() { 0 } else { 0 })
        .interact()?;

    match &source_map[selection] {
        PromptSource::Remote { path: _, content } => {
            println!("   ✓ Using existing remote instructions");
            Ok((content.clone(), false))
        }
        PromptSource::Template(name) => {
            // Find the variant file
            let variant = PROMPT_VARIANTS.iter()
                .find(|(_, desc)| *desc == name)
                .map(|(v, _)| v)
                .context("Unknown prompt variant")?;

            let prompts_src = PathBuf::from("prompts").join(variant);
            if !prompts_src.exists() {
                bail!("Prompt template not found: {}. Make sure 'prompts/{variant}' exists", prompts_src.display());
            }

            let local_content = fs::read_to_string(&prompts_src)
                .with_context(|| format!("Failed to read {}", prompts_src.display()))?;

            // Warn about overwriting if remote file exists
            if existing_remote.is_some() {
                let overwrite = Confirm::with_theme(theme)
                    .with_prompt(&format!(
                        "Remote instructions exist at {}. Overwrite?",
                        remote_instr_path
                    ))
                    .default(true)
                    .interact()?;

                if !overwrite {
                    println!("   ⚠ Skipping prompt upload — keeping existing remote instructions");
                    return Ok((existing_remote.unwrap(), false));
                }
            }

            println!("   ✓ Uploading {} → {}", variant, remote_instr_path);
            upload_instructions(
                client,
                base,
                &remote_instr_path,
                &local_content,
                user,
                password,
            ).await?;

            Ok((local_content, true))
        }
    }
}

/// Internal representation of a prompt source choice.
enum PromptSource {
    Remote { #[allow(dead_code)] path: String, content: String },
    Template(String),
}

/// Upload instructions content to WebDAV.
async fn upload_instructions(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    content: &str,
    user: &str,
    password: &str,
) -> Result<()> {
    let url = format!("{}/{}", base, path);
    let resp = client
        .put(&url)
        .basic_auth(user, Some(password))
        .body(content.to_string())
        .send()
        .await
        .with_context(|| format!("Failed to upload instructions to {}", path))?;

    if !resp.status().is_success() {
        anyhow::bail!(
            "Failed to upload instructions: {} {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        );
    }

    Ok(())
}
