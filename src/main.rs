use std::fs::read;
use std::sync::Arc;
use anyhow::Context;
use grammers_client::{Client, SenderPool};
use grammers_client::update::Update;
use grammers_session::storages::SqliteSession;
use grammers_tl_types::enums::{Document, DocumentAttribute, InputBotInlineResult, InputDocument, InputPeer, InputStickerSet, MessageMedia};
use grammers_tl_types::functions::messages::UploadMedia;
use grammers_tl_types::types::{DocumentAttributeImageSize, DocumentAttributeSticker, InputBotInlineMessageMediaAuto, InputBotInlineResultDocument, InputMediaUploadedDocument, MessageMediaDocument};
use skia_safe::{surfaces, Canvas, Color4f, ColorSpace, Data, EncodedImageFormat, Font, FontMgr, Image, Paint, TextBlob, Typeface};
use time::format_description::well_known::Iso8601;
use tokio::{runtime, task};
use tokio_util::task::AbortOnDropHandle;
use tracing_subscriber::{fmt, EnvFilter};
use tracing_subscriber::fmt::time::LocalTime;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const DIGITS: &str = "一二三四五六七八九";
const EXPONENTS: &str = "十百千";
const ZERO_MARK: char = '零';
const MYRIAD_MARK: &str = "万";
const TWO_MARK_FOR_THOUSANDS: char = '两';
const CHINESE_SUFFIX: &str = "社会信用";
const LATIN_SUFFIX_SHORT: &str = "Soc. Credit";
const LATIN_SUFFIX_FULL: &str = "Social Credit";

struct Typefaces {
    cjk: Typeface,
    latin: Typeface
}

fn format_latin_number(number: i32) -> Option<String>  {
    let abs = number.abs();

    if abs == 0 || abs >= 100000000 {
        return None;
    }

    let max_exp = {
        let mut cur = abs;
        let mut max_exp = 0;

        while cur > 0 && (cur % 10 == 0) {
            cur /= 10;
            max_exp += 1
        }

        max_exp
    };

    match max_exp / 3 {
        0 => Some(abs.to_string()),
        1 => Some((abs / 1000).to_string() + "k"),
        2 => Some((abs / 1000000).to_string() + "m"),
        _ => None
    }
}

fn format_chinese_number(number: i32) -> Option<String> {
    let mut abs = number.abs();

    if abs == 0 || abs >= 100000000 /*一亿*/ {
        return None
    }

    if abs > 10000 /*一万*/ {
        let lower_part = {
            let lower_part_int = abs % 10000;

            if lower_part_int == 0 {
                "".into()
            } else {
                format_chinese_number(lower_part_int)?
            }
        };

        let upper_part = {
            let upper_part_int = abs / 10000;
            format_chinese_number(upper_part_int)?
        };

        return Some(format!("{}{}{}", upper_part, MYRIAD_MARK, lower_part))
    }

    let mut exp = 0;
    let mut result: String = "".into();

    while abs > 0 {
        let digit = abs % 10;

        if digit == 0 {
            if !result.is_empty() && result.chars().nth(0).unwrap() != ZERO_MARK {
                result = ZERO_MARK.to_string() + &result
            }
        } else {
            let digit_char = match exp {
                3 if digit == 2 => TWO_MARK_FOR_THOUSANDS,
                _ => DIGITS.chars().nth((digit - 1) as usize)?
            };

            let exponent = if exp == 0 { "".into() } else { EXPONENTS.chars().nth(exp - 1)?.to_string() };

            result = format!("{}{}{}", digit_char, exponent, result);
        }

        abs /= 10;
        exp += 1;
    }

    Some(result)
}

fn render(base: Image, latin_number: String, chinese_number: String, typefaces: &Typefaces) -> Option<Vec<u8>> {
    let mut surface = surfaces::raster_n32_premul((512, 174))?;
    let canvas = surface.canvas();

    let srgb = ColorSpace::new_srgb();
    let white_paint = Paint::new(Color4f::new(1.0, 1.0, 1.0, 1.0), &srgb);
    let black_paint = Paint::new(Color4f::new(0.0, 0.0, 0.0, 1.0), &srgb);

    let render_shadowed = |canvas: &Canvas, text: String, font: &Font, x: i32, y: i32| -> Option<()> {
        let tl = TextBlob::from_text(&text, &font)?;

        canvas.draw_text_blob(&tl, (x + 4, y + 4), &black_paint);
        canvas.draw_text_blob(&tl, (x, y), &white_paint);

        Some(())
    };

    canvas.draw_image(base, (0, 0), None);

    let cjk_font_large = Font::new(&typefaces.cjk, Some(40.0.into()));
    let cjk_font_medium = Font::new(&typefaces.cjk, Some(36.0.into()));
    let cjk_font_small = Font::new(&typefaces.cjk, Some(32.0.into()));
    let cjk_font_pico = Font::new(&typefaces.cjk, Some(28.0.into()));

    let latin_y_comp = match chinese_number.chars().count() {
        _4 if _4 <= 4 => {
            render_shadowed(canvas, chinese_number + CHINESE_SUFFIX, &cjk_font_large, 160, 140);
            0
        }
        5 => {
            render_shadowed(canvas, chinese_number + CHINESE_SUFFIX, &cjk_font_medium, 160, 140);
            0
        }
        6 => {
            render_shadowed(canvas, chinese_number + CHINESE_SUFFIX, &cjk_font_small, 160, 140);
            0
        }
        7 => {
            render_shadowed(canvas, chinese_number + CHINESE_SUFFIX, &cjk_font_pico, 160, 135);
            10
        }
        8 | 9 | 10 | 11 => {
            render_shadowed(canvas, chinese_number, &cjk_font_pico, 160, 110);
            render_shadowed(canvas, CHINESE_SUFFIX.into(), &cjk_font_pico, 160, 145);
            0
        }
        _ => {
            let mut split_position = (chinese_number.chars().count() + CHINESE_SUFFIX.chars().count()) / 2;
            let first_wrapped_char = chinese_number.chars().nth(split_position)?;

            if !DIGITS.contains(first_wrapped_char) && first_wrapped_char != ZERO_MARK && first_wrapped_char != TWO_MARK_FOR_THOUSANDS {
                split_position += 1; // try not to break periods
            }

            let (lp, rp) = chinese_number.split_at(split_position);
            render_shadowed(canvas, lp.into(), &cjk_font_pico, 160, 110);
            render_shadowed(canvas, rp.to_string() + CHINESE_SUFFIX, &cjk_font_pico, 160, 145);
            0
        }
    };

    let latin_font_large = Font::new(&typefaces.latin, Some(29.0.into()));
    let latin_font_small = Font::new(&typefaces.latin, Some(24.0.into()));

    // render latin number
    let latin_suffix = if latin_number.chars().count() > 7 { LATIN_SUFFIX_SHORT } else { LATIN_SUFFIX_FULL };
    let (latin_font, latin_y) = if latin_number.chars().count() > 4 { (latin_font_small, 75) } else { (latin_font_large, 80) };

    render_shadowed(canvas, latin_number + " " + latin_suffix, &latin_font, 160, latin_y + latin_y_comp);

    let image = surface.image_snapshot();

    let data = image.encode(None, EncodedImageFormat::WEBP, Some(90))?;
    Some(data.as_bytes().to_vec())
}

fn render_number(orig_number: i32, sig: &str, base: Image, typefaces: &Typefaces) -> Option<Vec<u8>> {
    let chinese_number = format_chinese_number(orig_number)?;
    let latin_number = format_latin_number(orig_number)?;

    render(base, sig.to_string() + latin_number.as_str(), sig.to_string() + chinese_number.as_str(), typefaces)
}

fn render_raw_number(amount: i32, typefaces: &Typefaces) -> Option<Vec<u8>> {
    if amount == 0 {
        return None;
    }

    if amount < 0 {
        let minus = Image::from_encoded(Data::new_copy(&read("3rdparty/minus.png").ok()?))?;
        render_number(amount, "-", minus, typefaces)
    } else {
        let plus = Image::from_encoded(Data::new_copy(&read("3rdparty/plus.png").ok()?))?;
        render_number(amount, "+", plus, typefaces)
    }
}

async fn handle_update(client: Client, update: Update, typefaces: &Typefaces) -> anyhow::Result<()> {
    match update {
        Update::NewMessage(message) if !message.outgoing() => {
            let Some(chat) = message.peer() else {
                return Ok(())
            };

            println!("Responding to {}", chat.name().unwrap());
            // todo: excess unwraps do something about please omg
            client.send_message(message.peer_ref().await.unwrap().unwrap(), message.text()).await?;
        }
        Update::InlineQuery(query) => {
            println!("Query {}", query.text());

            let number: i32 = query.text().parse()?;
            let picture = render_raw_number(number, typefaces).context("Failed to render font")?;
            let mut cursor = std::io::Cursor::new(&picture);

            let file = client.upload_stream(&mut cursor, picture.len(), "sticker.webp".into()).await?;
            let uploaded = client.invoke(&UploadMedia {
                business_connection_id: None,
                media: InputMediaUploadedDocument {
                    nosound_video: false,
                    file: file.raw,
                    thumb: None,
                    ttl_seconds: None,
                    mime_type: "image/webp".into(),
                    attributes: vec![
                        DocumentAttribute::ImageSize(DocumentAttributeImageSize {
                            w: 512,
                            h: 174,
                        }),
                        DocumentAttribute::Sticker(DocumentAttributeSticker {
                            mask: false,
                            alt: "😀".into(),
                            stickerset: InputStickerSet::Empty,
                            mask_coords: None,
                        }),
                    ],
                    spoiler: false,
                    force_file: false,
                    stickers: None,
                    video_cover: None,
                    video_timestamp: None,
                }.into(),
                peer: InputPeer::PeerSelf
            }).await?;

            if let MessageMedia::Document(MessageMediaDocument { document: Some(Document::Document(d)), .. }) = uploaded {
                let x = grammers_tl_types::types::InputDocument {
                    id: d.id,
                    access_hash: d.access_hash,
                    file_reference: d.file_reference
                };

                query.answer(
                    vec![
                        InputBotInlineResult::Document(
                            InputBotInlineResultDocument {
                                document: InputDocument::Document(x),
                                title: Some("title".into()),
                                id: "1".into(),
                                r#type: "sticker".into(),
                                description: Some("asd".into()),
                                send_message: InputBotInlineMessageMediaAuto {
                                    invert_media: false,
                                    message: "wtf".into(),
                                    entities: None,
                                    reply_markup: None
                                }.into()
                            }
                        )
                    ]
                ).send().await?;
            }
        }
        _ => {}
    }

    Ok(())
}

async fn async_main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_timer(LocalTime::new(Iso8601::DATE_TIME))
                .with_ansi(true)
        )
        .with(
            EnvFilter::builder()
                .with_default_directive(tracing::Level::DEBUG.into())
                .with_env_var("SOC_LOG")
                .from_env_lossy()
        )
        .init();

    dotenvy::dotenv()?;

    let font_mgr = FontMgr::new();

    let cjk_typeface_data = read("3rdparty/BIZ-UDGothicR.ttc")?;
    let cjk_typeface = font_mgr.new_from_data(&cjk_typeface_data, None).expect("where font");

    let latin_typeface_data = read("3rdparty/VCR_OSD_MONO_1.001.ttf")?;
    let latin_typeface = font_mgr.new_from_data(&latin_typeface_data, None).expect("where font");

    let typefaces = Typefaces {
        latin: latin_typeface,
        cjk: cjk_typeface,
    };
    let typefaces = Arc::new(typefaces);

    let api_id = dotenvy::var("API_ID").expect("please provide API_ID").parse().expect("API_ID should be an integer");
    let api_hash = dotenvy::var("API_HASH").expect("please provide API_HASH");

    let token = dotenvy::var("BOT_TOKEN").expect("please provide BOT_TOKEN");

    println!("Connecting to Telegram...");

    let session = Arc::new(SqliteSession::open("bot777.session").await?);
    let pool = SenderPool::new(Arc::clone(&session), api_id);

    let client = Client::new(pool.handle);

    let _pool = AbortOnDropHandle::new(tokio::spawn(pool.runner.run()));

    if !client.is_authorized().await? {
        println!("Signing in...");
        client.bot_sign_in(&token, &api_hash).await?;
        println!("Signed in!");
    }

    println!("Waiting for messages...");

    let mut updates = client.stream_updates(pool.updates, Default::default()).await.unwrap();

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                println!("Received Ctrl-C!");
                break;
            }

            upd = updates.next() => {
                match upd {
                    Ok(upd) => {
                        let client = client.clone();
                        let typefaces = typefaces.clone();
    
                        task::spawn(async move {
                            let r = handle_update(client, upd, &typefaces).await;

                            if let Err(r) = r {
                                tracing::error!("Failed to handle update: {}", r);
                            }
                        });
                    },
                    Err(e) => {
                        eprintln!("Failed to get update: {e}");
                        break;
                    }
                }
            }
        }
    }

    Ok(())
}

fn main() -> anyhow::Result<()> {
    runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async_main())
}