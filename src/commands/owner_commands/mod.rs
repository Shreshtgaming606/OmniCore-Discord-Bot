pub mod all_servers;
pub mod create_invite;
pub mod kick_self;
use crate::CustomContext;

/// Autocompletes guild choices by ID or name (case-insensitive substring match).
async fn autocomplete_guild(
    ctx: CustomContext<'_>,
    partial: &str,
) -> impl Iterator<Item = poise::serenity_prelude::AutocompleteChoice> {
    let partial_lower = partial.to_lowercase();

    let guilds = ctx.http().get_guilds(None, None).await.unwrap_or_default();

    guilds
        .into_iter()
        .filter_map(|guild| {
            ctx.cache()
                .guild(guild.id)
                .map(|cached| (guild.id, cached.name.clone()))
        })
        .filter(move |(id, name)| {
            name.to_lowercase().contains(&partial_lower) || id.to_string().contains(&partial_lower)
        })
        // Discord caps autocomplete results at 25; take the best 25 matches.
        .take(25)
        .map(|(id, name)| {
            poise::serenity_prelude::AutocompleteChoice::new(
                format!("{} ({})", name, id),
                id.to_string(),
            )
        })
        .collect::<Vec<_>>()
        .into_iter()
}
