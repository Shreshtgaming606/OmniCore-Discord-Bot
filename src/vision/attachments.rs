use super::VisionError;
use image::{ImageFormat, ImageReader, Limits};
use poise::serenity_prelude::Attachment;
use reqwest::{Client, StatusCode, Url, redirect::Policy};
use std::{collections::HashSet, io::Cursor, sync::OnceLock, time::Duration};
use zeroize::Zeroizing;

pub const HARD_MAX_IMAGES: usize = 3;
const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;
const MAX_PIXELS: u64 = 20_000_000;
const MAX_DIMENSION: u32 = 6_000;

pub struct VisionImage {
    pub mime: &'static str,
    pub bytes: Zeroizing<Vec<u8>>,
}

fn is_candidate(attachment: &Attachment) -> bool {
    attachment.width.is_some()
        || attachment.height.is_some()
        || attachment
            .content_type
            .as_deref()
            .is_some_and(|mime| mime.starts_with("image/"))
        || attachment
            .filename
            .rsplit_once('.')
            .is_some_and(|(_, extension)| {
                matches!(
                    extension.to_ascii_lowercase().as_str(),
                    "png" | "jpg" | "jpeg" | "webp" | "gif"
                )
            })
}

/// Only attachments from the invoking message and its one direct reply target
/// are passed here. A missing candidate does not authorize looking elsewhere.
pub fn select(
    current: &[Attachment],
    direct_reply: &[Attachment],
    max_images: usize,
) -> Result<Vec<Attachment>, VisionError> {
    let mut seen = HashSet::new();
    let selected: Vec<_> = current
        .iter()
        .chain(direct_reply)
        .filter(|attachment| is_candidate(attachment))
        .filter(|attachment| seen.insert(attachment.id))
        .cloned()
        .collect();
    if selected.is_empty() {
        return Err(if current.is_empty() && direct_reply.is_empty() {
            VisionError::NoImage
        } else {
            VisionError::UnsupportedFormat
        });
    }
    if selected.len() > max_images.clamp(1, HARD_MAX_IMAGES) {
        return Err(VisionError::TooManyImages);
    }
    Ok(selected)
}

pub fn approved_url(raw: &str) -> Result<Url, VisionError> {
    let url = Url::parse(raw).map_err(|_| VisionError::DownloadFailed)?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() != Some(443)
        || !matches!(
            url.host_str(),
            Some("cdn.discordapp.com" | "media.discordapp.net")
        )
        || !(url.path().starts_with("/attachments/")
            || url.path().starts_with("/ephemeral-attachments/"))
    {
        return Err(VisionError::DownloadFailed);
    }
    Ok(url)
}

fn download_client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(12))
            .redirect(Policy::none())
            .no_proxy()
            .user_agent("OmniCore/1.0 (+https://discordbot.omnicoretech.org)")
            .build()
            .expect("Discord attachment client")
    })
}

fn status_error(status: StatusCode) -> VisionError {
    match status {
        StatusCode::FORBIDDEN | StatusCode::NOT_FOUND | StatusCode::GONE => VisionError::Expired,
        _ => VisionError::DownloadFailed,
    }
}

pub async fn download(attachment: &Attachment) -> Result<VisionImage, VisionError> {
    if attachment.size as usize > MAX_FILE_BYTES {
        return Err(VisionError::TooLarge);
    }
    let url = approved_url(&attachment.url)?;
    let mut response = download_client()
        .get(url)
        .send()
        .await
        .map_err(|_| VisionError::DownloadFailed)?;
    if !response.status().is_success() {
        return Err(status_error(response.status()));
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_FILE_BYTES as u64)
    {
        return Err(VisionError::TooLarge);
    }
    let header_mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|header| header.to_str().ok())
        .map(str::to_owned);
    if header_mime
        .as_deref()
        .is_some_and(|mime| mime.starts_with("text/"))
    {
        return Err(VisionError::Expired);
    }
    let mut bytes = Vec::with_capacity((attachment.size as usize).min(MAX_FILE_BYTES));
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| VisionError::DownloadFailed)?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_FILE_BYTES {
            return Err(VisionError::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    validate(
        bytes,
        attachment.content_type.as_deref(),
        header_mime.as_deref(),
    )
}

pub fn validate(
    bytes: Vec<u8>,
    declared_mime: Option<&str>,
    response_mime: Option<&str>,
) -> Result<VisionImage, VisionError> {
    if bytes.is_empty() || bytes.len() > MAX_FILE_BYTES {
        return Err(VisionError::TooLarge);
    }
    let format = image::guess_format(&bytes).map_err(|_| VisionError::UnsupportedFormat)?;
    let expected = match format {
        ImageFormat::Png => "image/png",
        ImageFormat::Jpeg => "image/jpeg",
        ImageFormat::WebP => "image/webp",
        ImageFormat::Gif => "image/gif",
        _ => return Err(VisionError::UnsupportedFormat),
    };
    for mime in [declared_mime, response_mime].into_iter().flatten() {
        let mime = mime.split(';').next().unwrap_or("").trim();
        if mime.starts_with("image/") && mime != expected {
            return Err(VisionError::InvalidImage);
        }
    }
    let reader = ImageReader::with_format(Cursor::new(bytes.as_slice()), format);
    let (width, height) = reader
        .into_dimensions()
        .map_err(|_| VisionError::InvalidImage)?;
    if width == 0
        || height == 0
        || width > MAX_DIMENSION
        || height > MAX_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        return Err(VisionError::TooLarge);
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes.as_slice()), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|_| VisionError::InvalidImage)?;
    if format == ImageFormat::Gif {
        // The decoder provides the first representative frame. PNG is supported
        // by all three providers and avoids sending an animated GIF to OpenAI.
        let mut png = Cursor::new(Vec::new());
        decoded
            .write_to(&mut png, ImageFormat::Png)
            .map_err(|_| VisionError::InvalidImage)?;
        let png = png.into_inner();
        if png.len() > MAX_FILE_BYTES {
            return Err(VisionError::TooLarge);
        }
        Ok(VisionImage {
            mime: "image/png",
            bytes: Zeroizing::new(png),
        })
    } else {
        Ok(VisionImage {
            mime: expected,
            bytes: Zeroizing::new(bytes),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use poise::serenity_prelude::Attachment;

    fn attachment(id: u64, filename: &str, mime: &str) -> Attachment {
        serde_json::from_value(serde_json::json!({
            "id": id.to_string(), "filename": filename, "size": 100,
            "url": format!("https://cdn.discordapp.com/attachments/1/2/{filename}"),
            "proxy_url": format!("https://media.discordapp.net/attachments/1/2/{filename}"),
            "content_type": mime, "height": 1, "width": 1
        }))
        .unwrap()
    }

    #[test]
    fn selects_only_current_and_exact_reply_images_with_limit() {
        let current = [attachment(1, "one.png", "image/png")];
        let replied = [attachment(2, "two.jpg", "image/jpeg")];
        assert_eq!(select(&current, &replied, 3).unwrap().len(), 2);
        assert_eq!(
            select(&current, &replied, 1).err(),
            Some(VisionError::TooManyImages)
        );
        assert_eq!(select(&[], &[], 3).err(), Some(VisionError::NoImage));
        let mut document = attachment(3, "notes.pdf", "application/pdf");
        document.width = None;
        document.height = None;
        let unsupported = [document];
        assert_eq!(
            select(&unsupported, &[], 3).err(),
            Some(VisionError::UnsupportedFormat)
        );
    }

    #[test]
    fn rejects_non_discord_and_cross_channel_urls() {
        assert!(approved_url("http://127.0.0.1/attachments/a").is_err());
        assert!(approved_url("https://evil.example/attachments/a").is_err());
        assert!(approved_url("https://cdn.discordapp.com@evil.example/attachments/a").is_err());
        assert!(approved_url("https://cdn.discordapp.com/other/a").is_err());
        assert!(approved_url("https://cdn.discordapp.com/attachments/1/2/3.png?ex=abc").is_ok());
    }

    #[test]
    fn rejects_invalid_or_mislabeled_images() {
        assert!(matches!(
            validate(b"not an image".to_vec(), Some("image/png"), None),
            Err(VisionError::UnsupportedFormat)
        ));
        let corrupt_png = vec![137, 80, 78, 71, 13, 10, 26, 10, 0, 0];
        assert!(matches!(
            validate(corrupt_png, Some("image/png"), None),
            Err(VisionError::InvalidImage)
        ));
        assert_eq!(status_error(StatusCode::FORBIDDEN), VisionError::Expired);
    }

    #[test]
    fn decodes_real_png_and_transcodes_gif_first_frame() {
        let mut png = Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(2, 2)
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        let image = validate(png.into_inner(), Some("image/png"), Some("image/png")).unwrap();
        assert_eq!(image.mime, "image/png");
        let mut gif = Vec::new();
        image::codecs::gif::GifEncoder::new(&mut gif)
            .encode(&[255, 0, 0, 255], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        let converted = validate(gif, Some("image/gif"), None).unwrap();
        assert_eq!(converted.mime, "image/png");
        assert_eq!(
            image::guess_format(&converted.bytes).unwrap(),
            ImageFormat::Png
        );
    }
}
