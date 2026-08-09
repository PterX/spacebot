//! `spacebot cron` — cron job management over the control API.

use super::client::{self, ApiClient};
use super::output;
use clap::Subcommand;
use spacebot::api::cron::{CronActionResponse, CronListResponse};

#[derive(Subcommand)]
pub enum CronCommand {
    /// List cron jobs for an agent
    List {
        /// Agent ID
        agent: String,
    },
    /// Create or update a cron job
    Set {
        /// Agent ID
        agent: String,
        /// Cron job ID
        id: String,
        /// Prompt to run on each execution
        #[arg(short, long)]
        prompt: String,
        /// Delivery target (adapter:target)
        #[arg(short, long)]
        target: String,
        /// 5-field cron expression (takes precedence over --interval)
        #[arg(short, long)]
        cron: Option<String>,
        /// Interval between runs in seconds (default 3600)
        #[arg(short, long)]
        interval: Option<u64>,
        /// Start of the active hours window (0-23)
        #[arg(long)]
        start_hour: Option<u8>,
        /// End of the active hours window (0-23)
        #[arg(long)]
        end_hour: Option<u8>,
        /// Save the job disabled
        #[arg(long)]
        disabled: bool,
        /// Disable the job after its first run
        #[arg(long)]
        run_once: bool,
        /// Execution timeout in seconds
        #[arg(long)]
        timeout: Option<u64>,
    },
    /// Delete a cron job
    Delete {
        /// Agent ID
        agent: String,
        /// Cron job ID
        id: String,
    },
    /// Enable or disable a cron job
    Toggle {
        /// Agent ID
        agent: String,
        /// Cron job ID
        id: String,
        /// Enable the job
        #[arg(long, conflicts_with = "off")]
        on: bool,
        /// Disable the job
        #[arg(long)]
        off: bool,
    },
    /// Trigger a cron job immediately
    Trigger {
        /// Agent ID
        agent: String,
        /// Cron job ID
        id: String,
    },
    /// Show cron execution history
    Executions {
        /// Agent ID
        agent: String,
        /// Filter by cron job ID
        #[arg(short, long)]
        cron_id: Option<String>,
        /// Maximum number of executions to return
        #[arg(short, long, default_value_t = 50)]
        limit: i64,
    },
}

pub async fn run(ctx: &super::Context, cron_cmd: CronCommand) -> anyhow::Result<()> {
    let client = ApiClient::from_context(ctx)?;

    match cron_cmd {
        CronCommand::List { agent } => {
            let value = client
                .get(&format!(
                    "agents/cron?agent_id={}",
                    urlencoding::encode(&agent)
                ))
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let response: CronListResponse = client::parse(value)?;
            if response.jobs.is_empty() {
                eprintln!("No cron jobs for {agent}.");
                return Ok(());
            }
            eprintln!("Timezone: {}", response.timezone);
            let rows: Vec<Vec<String>> = response
                .jobs
                .iter()
                .map(|job| {
                    let schedule = job
                        .cron_expr
                        .clone()
                        .unwrap_or_else(|| format!("every {}s", job.interval_secs));
                    let last_run = job
                        .last_executed_at
                        .as_deref()
                        .map(output::short_timestamp)
                        .unwrap_or_else(|| "-".to_string());
                    vec![
                        job.id.clone(),
                        schedule,
                        if job.enabled { "yes" } else { "no" }.to_string(),
                        job.delivery_target.clone(),
                        job.execution_success_count.to_string(),
                        job.execution_failure_count.to_string(),
                        last_run,
                    ]
                })
                .collect();
            output::table(
                &[
                    "ID", "SCHEDULE", "ENABLED", "TARGET", "OK", "FAIL", "LAST RUN",
                ],
                &rows,
            );
            Ok(())
        }
        CronCommand::Set {
            agent,
            id,
            prompt,
            target,
            cron,
            interval,
            start_hour,
            end_hour,
            disabled,
            run_once,
            timeout,
        } => {
            let mut body = serde_json::json!({
                "agent_id": agent,
                "id": id,
                "prompt": prompt,
                "delivery_target": target,
                "enabled": !disabled,
                "run_once": run_once,
            });
            if let Some(cron) = &cron {
                body["cron_expr"] = serde_json::json!(cron);
            }
            if let Some(interval) = interval {
                body["interval_secs"] = serde_json::json!(interval);
            }
            if let Some(start_hour) = start_hour {
                body["active_start_hour"] = serde_json::json!(start_hour);
            }
            if let Some(end_hour) = end_hour {
                body["active_end_hour"] = serde_json::json!(end_hour);
            }
            if let Some(timeout) = timeout {
                body["timeout_secs"] = serde_json::json!(timeout);
            }

            let value = client.post("agents/cron", &body).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let result: CronActionResponse = client::parse(value)?;
            eprintln!("{}", result.message);
            Ok(())
        }
        CronCommand::Delete { agent, id } => {
            let value = client
                .delete(&format!(
                    "agents/cron?agent_id={}&cron_id={}",
                    urlencoding::encode(&agent),
                    urlencoding::encode(&id)
                ))
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let result: CronActionResponse = client::parse(value)?;
            eprintln!("{}", result.message);
            Ok(())
        }
        CronCommand::Toggle { agent, id, on, off } => {
            if on == off {
                anyhow::bail!("pass exactly one of --on or --off");
            }
            let body = serde_json::json!({
                "agent_id": agent,
                "cron_id": id,
                "enabled": on,
            });
            let value = client.put("agents/cron/toggle", &body).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let result: CronActionResponse = client::parse(value)?;
            eprintln!("{}", result.message);
            Ok(())
        }
        CronCommand::Trigger { agent, id } => {
            let body = serde_json::json!({
                "agent_id": agent,
                "cron_id": id,
            });
            let value = client.post("agents/cron/trigger", &body).await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            let result: CronActionResponse = client::parse(value)?;
            eprintln!("{}", result.message);
            Ok(())
        }
        CronCommand::Executions {
            agent,
            cron_id,
            limit,
        } => {
            let mut query = vec![
                format!("agent_id={}", urlencoding::encode(&agent)),
                format!("limit={limit}"),
            ];
            if let Some(cron_id) = &cron_id {
                query.push(format!("cron_id={}", urlencoding::encode(cron_id)));
            }

            let value = client
                .get(&format!("agents/cron/executions?{}", query.join("&")))
                .await?;
            if ctx.json {
                output::json(&value);
                return Ok(());
            }
            // The execution entry type is serialize-only in the library,
            // so render straight from the response value.
            let executions = value["executions"].as_array().cloned().unwrap_or_default();
            if executions.is_empty() {
                eprintln!("No executions recorded.");
                return Ok(());
            }
            let rows: Vec<Vec<String>> = executions
                .iter()
                .map(|entry| {
                    let execution = if entry["execution_succeeded"].as_bool().unwrap_or(false) {
                        "ok"
                    } else {
                        "failed"
                    };
                    let delivery = if !entry["delivery_attempted"].as_bool().unwrap_or(false) {
                        "skipped"
                    } else {
                        match entry["delivery_succeeded"].as_bool() {
                            Some(true) => "ok",
                            Some(false) => "failed",
                            None => "-",
                        }
                    };
                    let detail = entry["execution_error"]
                        .as_str()
                        .or_else(|| entry["delivery_error"].as_str())
                        .or_else(|| entry["result_summary"].as_str())
                        .unwrap_or("");
                    vec![
                        output::short_timestamp(entry["executed_at"].as_str().unwrap_or("")),
                        entry["cron_id"].as_str().unwrap_or("-").to_string(),
                        execution.to_string(),
                        delivery.to_string(),
                        output::truncate(detail, 60),
                    ]
                })
                .collect();
            output::table(
                &["EXECUTED", "CRON", "EXECUTION", "DELIVERY", "DETAIL"],
                &rows,
            );
            Ok(())
        }
    }
}
