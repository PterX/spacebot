#!/bin/sh
set -e

mkdir -p "$SPACEBOT_DIR"
mkdir -p "$SPACEBOT_DIR/tools/bin"

# Provision the repository used by execution/review agents. The runtime image
# contains the compiled application, not its source checkout, so without this
# step an agent can be told to work in Spacebot but has no files or Git history
# to inspect. Keep the checkout in its own persistent volume so restarts do not
# discard agent work.
if [ "${SPACEBOT_REPO_ENABLED:-true}" = "true" ]; then
    repo_dir="${SPACEBOT_REPO_DIR:-/workspace/spacebot}"
    repo_url="${SPACEBOT_REPO_URL:-https://github.com/mdcnick/spacebot.git}"
    repo_ref="${SPACEBOT_REPO_REF:-local/spacebot-fix/2026-08-05-toggle-radix-state-fix}"
    mkdir -p "$(dirname "$repo_dir")"
    if [ -d "$repo_dir/.git" ]; then
        actual_url=$(git -C "$repo_dir" remote get-url origin 2>/dev/null || true)
        if [ "$actual_url" != "$repo_url" ]; then
            echo "Spacebot repository origin mismatch in $repo_dir: expected $repo_url, found ${actual_url:-<missing>}" >&2
            exit 1
        fi

        # Keep a persistent checkout current without overwriting agent work.
        # Only fast-forward a clean checkout; local commits or edits remain
        # untouched and require an explicit operator decision.
        git -C "$repo_dir" fetch origin "$repo_ref"
        current_ref=$(git -C "$repo_dir" symbolic-ref --short -q HEAD || true)
        if [ "$current_ref" != "$repo_ref" ]; then
            if ! git -C "$repo_dir" diff --quiet || ! git -C "$repo_dir" diff --cached --quiet; then
                echo "Spacebot repository has uncommitted changes; cannot switch from $current_ref to $repo_ref" >&2
                exit 1
            fi
            git -C "$repo_dir" checkout -B "$repo_ref" "origin/$repo_ref"
        elif git -C "$repo_dir" diff --quiet && git -C "$repo_dir" diff --cached --quiet; then
            git -C "$repo_dir" merge --ff-only "origin/$repo_ref" 2>/dev/null || true
        fi
    else
        if [ -e "$repo_dir" ] && [ "$(find "$repo_dir" -mindepth 1 -maxdepth 1 -print -quit)" ]; then
            echo "Spacebot repository directory is not empty: $repo_dir" >&2
            exit 1
        fi
        if [ -d "$repo_dir" ]; then
            rmdir "$repo_dir"
        fi
        git clone --branch "$repo_ref" --single-branch "$repo_url" "$repo_dir"
    fi
fi

# Generate config.toml from environment variables when no config file exists.
# Once a config.toml is present on the volume, this is skipped entirely.
if [ ! -f "$SPACEBOT_DIR/config.toml" ]; then
    cat > "$SPACEBOT_DIR/config.toml" <<EOF
[api]
bind = "::"

[llm]
anthropic_key = "env:ANTHROPIC_API_KEY"
openai_key = "env:OPENAI_API_KEY"
openrouter_key = "env:OPENROUTER_API_KEY"
EOF

    # Discord adapter
    if [ -n "$DISCORD_BOT_TOKEN" ]; then
        cat >> "$SPACEBOT_DIR/config.toml" <<EOF

[messaging.discord]
enabled = true
token = "env:DISCORD_BOT_TOKEN"
EOF
        if [ -n "$DISCORD_DM_ALLOWED_USERS" ]; then
            # Comma-separated user IDs -> TOML array
            DM_ARRAY=$(echo "$DISCORD_DM_ALLOWED_USERS" | sed 's/[[:space:]]//g' | sed 's/,/", "/g')
            cat >> "$SPACEBOT_DIR/config.toml" <<EOF
dm_allowed_users = ["$DM_ARRAY"]
EOF
        fi
    fi

    # Telegram adapter
    if [ -n "$TELEGRAM_BOT_TOKEN" ]; then
        cat >> "$SPACEBOT_DIR/config.toml" <<EOF

[messaging.telegram]
enabled = true
token = "env:TELEGRAM_BOT_TOKEN"
EOF
    fi

    # Webhook adapter
    if [ -n "$WEBHOOK_ENABLED" ]; then
        cat >> "$SPACEBOT_DIR/config.toml" <<EOF

[messaging.webhook]
enabled = true
bind = "0.0.0.0"
EOF
    fi

    # Default agent
    cat >> "$SPACEBOT_DIR/config.toml" <<EOF

[[agents]]
id = "main"
default = true
EOF

    # Discord binding
    if [ -n "$DISCORD_GUILD_ID" ]; then
        cat >> "$SPACEBOT_DIR/config.toml" <<EOF

[[bindings]]
agent_id = "main"
channel = "discord"
guild_id = "$DISCORD_GUILD_ID"
EOF
        if [ -n "$DISCORD_CHANNEL_IDS" ]; then
            CH_ARRAY=$(echo "$DISCORD_CHANNEL_IDS" | sed 's/[[:space:]]//g' | sed 's/,/", "/g')
            cat >> "$SPACEBOT_DIR/config.toml" <<EOF
channel_ids = ["$CH_ARRAY"]
EOF
        fi
        if [ -n "$DISCORD_DM_ALLOWED_USERS" ]; then
            DM_ARRAY=$(echo "$DISCORD_DM_ALLOWED_USERS" | sed 's/[[:space:]]//g' | sed 's/,/", "/g')
            cat >> "$SPACEBOT_DIR/config.toml" <<EOF
dm_allowed_users = ["$DM_ARRAY"]
EOF
        fi
    fi

    # Telegram binding
    if [ -n "$TELEGRAM_CHAT_ID" ]; then
        cat >> "$SPACEBOT_DIR/config.toml" <<EOF

[[bindings]]
agent_id = "main"
channel = "telegram"
chat_id = "$TELEGRAM_CHAT_ID"
EOF
    fi

    echo "Generated config.toml from environment variables"
fi

exec "$@"
