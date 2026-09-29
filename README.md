# OmniCore Discord Bot

OmniCore Discord Bot is a multi-purpose Discord bot written in Rust. It uses the [`poise`](https://docs.rs/poise) command framework, MongoDB for persistence, and Ollama integration to bring AI-powered interactions to your Discord server.

## Features

- Slash commands and prefix commands
- Dynamic per-server command prefix
- Moderation tools
    - Ban / unban
    - Kick
    - Lock / unlock
    - Purge messages
    - Time-based actions
- Utility commands
    - Ping
    - Help
    - Info
    - User **(WIP)** / server / role utilities
    - Emoji **(WIP)** and role comparison helpers
- AI features
    - Responds when mentioned or replied to
    - Prompt management
    - AI approval / disapproval controls
    - Memory handling
    - Ollama integration
- Owner-only commands
    - Server listing
    - Invite creation
    - Self-kick utility

## Tech Stack

- **Language:** Rust
- **Discord framework:** `poise` + `serenity`
- **Database:** MongoDB
- **Logging:** `log` + `log4rs`
- **AI integration:** Ollama
- **Async runtime:** Tokio

## How It Works

The bot:
- registers commands globally on startup
- loads configuration from environment variables
- connects to MongoDB
- initializes Ollama integration
- listens for mentions and replies to trigger AI responses
- supports per-guild settings such as custom prefixes

## AI Usage (in code)
AI has been used to create parts or assist with the code. It is also used to create documentation.
### Policy
All code/documentation HAS to be reviewed by a real human.