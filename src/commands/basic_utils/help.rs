use crate::{CustomContext, Error, commands::basic_utils::prefix::get_prefix};

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
    //! Help command that lists all available commands.

    let prefix = get_prefix(
        ctx.guild_id()
            .unwrap_or(poise::serenity_prelude::GuildId::new(1)),
    )
    .await;

    let config = poise::builtins::HelpConfiguration {
        extra_text_at_bottom: &format!(
            "Type {}help command for more info on a command. Mention (@) the bot to talk to the AI.\
            \nIf you are trying to enter in text NOT using slash-commands, put `\"` around the text. This doesn't apply to commands that have only one text field\n\n\
            Do you want OmniCore AI to follow your own rules? Run change_prompt to add your own rules.\n\nAutoMod is enabled by default. Use /automod settings to view it; server owners can use /automod enable, disable, sensitivity, timeout, warnings, timeouts, notifications, and log-channel.",
            prefix
        ),
        include_description: true,
        ..Default::default()
    };
    poise::builtins::help(ctx, command.as_deref(), config).await?;
    Ok(())
}
