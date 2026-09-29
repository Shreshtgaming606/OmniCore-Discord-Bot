use crate::{CustomContext, Error, commands::basic_utils::prefix::get_prefix};
use poise::{
    CreateReply,
    serenity_prelude::{Colour, CreateEmbed, GuildId},
};
use std::collections::BTreeMap;

/// Shows the built-in help output for all commands or a specific command.
#[poise::command(
    prefix_command,
    track_edits,
    slash_command,
    description_localized("en-US", "Help command that lists all available commands."),
    broadcast_typing,
    category = "Utility"
)]
pub async fn help(
    ctx: CustomContext<'_>,
    #[description = "Specific command to show help about"] command: Option<String>,
) -> Result<(), Error> {
    let prefix = get_prefix(ctx.guild_id().unwrap_or(GuildId::new(1))).await;

    // per-command help is short, so the builtin is fine here.
    if let Some(cmd) = command.as_deref() {
        let config = poise::builtins::HelpConfiguration {
            include_description: true,
            ..Default::default()
        };
        poise::builtins::help(ctx, Some(cmd), config).await?;
        return Ok(());
    }

    // group visible commands by category
    let mut by_category: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for cmd in &ctx.framework().options().commands {
        if cmd.hide_in_help {
            continue;
        }
        let category = cmd.category.clone().unwrap_or_else(|| "Other".into());
        let desc = cmd.description.as_deref().unwrap_or("No description");
        by_category
            .entry(category)
            .or_default()
            .push(format!("`{}{}` - {}", prefix, cmd.name, desc));
    }

    let mut embed = CreateEmbed::new()
        .title("Commands")
        .description(format!(
            "Type `{prefix}help <command>` for more info on a command. Mention (@) the bot to talk to the AI.\n\n\
             If you're entering text **without** slash commands, put `\"` around the text. \
             This doesn't apply to commands with only one text field.\n\n\
             Want OmniCore AI to follow your own rules? Run `change_prompt` to add them."
        ))
        .color(Colour::from_rgb(88, 101, 242))
        .timestamp(chrono::Utc::now());

    // fields cap at 1024 chars, so split long categories across multiple fields
    for (category, lines) in by_category {
        let mut chunk = String::new();
        let mut first = true;
        for line in lines {
            if chunk.len() + line.len() + 1 > 1024 {
                let name = if first {
                    category.clone()
                } else {
                    format!("{category} (cont.)")
                };
                embed = embed.field(name, std::mem::take(&mut chunk), false);
                first = false;
            }
            chunk.push_str(&line);
            chunk.push('\n');
        }
        if !chunk.is_empty() {
            let name = if first {
                category.clone()
            } else {
                format!("{category} (cont.)")
            };
            embed = embed.field(name, chunk, false);
        }
    }

    ctx.send(CreateReply::default().embed(embed)).await?;
    Ok(())
}
