use std::convert::TryInto;
use futures_util::future::{select, Either};
use grammers_client::{Client, Config, InitParams, InputMessage, Update};
use grammers_session::Session;
use log;
use simple_logger::SimpleLogger;
use std::env;
use std::fs::read;
use std::pin::pin;
use grammers_client::client::bots::InlineResult;
use grammers_client::types::inline_query;
use grammers_client::types::inline_query::Article;
use grammers_tl_types::{enums, Serializable, types};
use grammers_tl_types::enums::{Document, InputBotInlineMessage, InputDocument, InputMedia, InputPeer, MessageMedia};
use grammers_tl_types::functions::messages::UploadMedia;
use grammers_tl_types::types::{InputBotInlineMessageMediaAuto, InputBotInlineMessageText, InputBotInlineResult, InputBotInlineResultDocument, InputMediaUploadedDocument, InputMediaUploadedPhoto, InputStickeredMediaDocument, MessageMediaDocument};
use skia_safe::{Canvas, Color4f, ColorSpace, Data, EncodedImageFormat, Font, Image, Paint, Point, Size, Surface, TextBlob, TextEncoding, Typeface};
use tokio::{runtime, task};
use tokio::io::AsyncRead;

type Result = std::result::Result<(), Box<dyn std::error::Error>>;

const SESSION_FILE: &str = "echo.session";

const DIGITS: &str = "一二三四五六七八九";
const EXPONENTS: &str = "十百千";
const ZERO_MARK: char = '零';
const MYRIAD_MARK: &str = "万";
const TWO_MARK_FOR_THOUSANDS: char = '两';
const CHINESE_SUFFIX: &str = "社会信用";
const LATIN_SUFFIX_SHORT: &str = "Soc. Credit";
const LATIN_SUFFIX_FULL: &str = "Social Credit";

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

fn render(base: Image, latin_number: String, chinese_number: String) -> Option<Vec<u8>> {
    let mut surface = Surface::new_raster_n32_premul((512, 174))?;
    let mut canvas = surface.canvas();

    let srgb = ColorSpace::new_srgb();
    let white_paint = Paint::new(Color4f::new(1.0, 1.0, 1.0, 1.0), &srgb);
    let black_paint = Paint::new(Color4f::new(0.0, 0.0, 0.0, 1.0), &srgb);

    let mut render_shadowed = |canvas: &mut Canvas, text: String, font: &Font, x: i32, y: i32| -> Option<()> {
        let tl = TextBlob::from_text(&text.to_bytes(), TextEncoding::UTF8, &font)?;

        canvas.draw_text_blob(&tl, (x + 4, y + 4), &black_paint);
        canvas.draw_text_blob(&tl, (x, y), &white_paint);

        Some(())
    };

    canvas.draw_image(base, (0, 0), None);

    let cjkTypefaceData = read("3rdparty/BIZ-UDGothicR.ttc").ok()?;
    let cjkTypeface = Typeface::from_data(Data::new_copy(&cjkTypefaceData), None)?;
    let cjkFontLarge = Font::new(&cjkTypeface, Some(40.0.into()));
    let cjkFontMedium = Font::new(&cjkTypeface, Some(36.0.into()));
    let cjkFontSmall = Font::new(&cjkTypeface, Some(32.0.into()));
    let cjkFontPico = Font::new(&cjkTypeface, Some(28.0.into()));

    let latinYComp = match chinese_number.chars().count() {
        _4 if _4 <= 4 => {
            render_shadowed(canvas, chinese_number + CHINESE_SUFFIX, &cjkFontLarge, 160, 140);
            0
        }
        5 => {
            render_shadowed(canvas, chinese_number + CHINESE_SUFFIX, &cjkFontMedium, 160, 140);
            0
        }
        6 => {
            render_shadowed(canvas, chinese_number + CHINESE_SUFFIX, &cjkFontSmall, 160, 140);
            0
        }
        7 => {
            render_shadowed(canvas, chinese_number + CHINESE_SUFFIX, &cjkFontPico, 160, 135);
            10
        }
        8 | 9 | 10 | 11 => {
            render_shadowed(canvas, chinese_number, &cjkFontPico, 160, 110);
            render_shadowed(canvas, CHINESE_SUFFIX.into(), &cjkFontPico, 160, 145);
            0
        }
        _ => {
            let mut splitPosition = (chinese_number.chars().count() + CHINESE_SUFFIX.chars().count()) / 2;
            let firstWrappedChar = chinese_number.chars().nth(splitPosition)?;

            if !DIGITS.contains(firstWrappedChar) && firstWrappedChar != ZERO_MARK && firstWrappedChar != TWO_MARK_FOR_THOUSANDS {
                splitPosition += 1; // try not to break periods
            }

            let (lp, rp) = chinese_number.split_at(splitPosition);
            render_shadowed(canvas, lp.into(), &cjkFontPico, 160, 110);
            render_shadowed(canvas, rp.to_string() + CHINESE_SUFFIX, &cjkFontPico, 160, 145);
            0
        }
    };

    let latinTypefaceData = read("3rdparty/VCR_OSD_MONO_1.001.ttf").ok()?;
    let latinTypeface = Typeface::from_data(Data::new_copy(&latinTypefaceData), None)?;
    let latinFontLarge = Font::new(&latinTypeface, Some(29.0.into()));
    let latinFontSmall = Font::new(&latinTypeface, Some(24.0.into()));

    // render latin number
    let latinSuffix = if latin_number.chars().count() > 7 { LATIN_SUFFIX_SHORT } else { LATIN_SUFFIX_FULL };
    let (latinFont, latinY) = if latin_number.chars().count() > 4 { (latinFontSmall, 75) } else { (latinFontLarge, 80) };

    render_shadowed(canvas, latin_number + " " + latinSuffix, &latinFont, 160, latinY + latinYComp);

    let image = surface.image_snapshot();
    let data = image.encode_to_data(EncodedImageFormat::WEBP)?;

    Some(data.as_bytes().to_bytes())
}

fn render_number(orig_number: i32, sig: &str, base: Image) -> Option<Vec<u8>> {
    let chinese_number = format_chinese_number(orig_number)?;
    let latin_number = format_latin_number(orig_number)?;

    render(base, sig.to_string() + latin_number.as_str(), sig.to_string() + chinese_number.as_str())
}

fn render_raw_number(amount: i32) -> Option<Vec<u8>> {
    if amount == 0 {
        return None;
    }

    if amount < 0 {
        let minus = Image::from_encoded(Data::new_copy(&read("3rdparty/minus.png").ok()?))?;
        render_number(amount, "-", minus)
    } else {
        let plus = Image::from_encoded(Data::new_copy(&read("3rdparty/plus.png").ok()?))?;
        render_number(amount, "+", plus)
    }
}

async fn handle_update(client: Client, update: Update) -> Result {
    match update {
        Update::NewMessage(message) if !message.outgoing() => {
            let chat = message.chat();
            println!("Responding to {}", chat.name());
            client.send_message(&chat, message.text()).await?;
        }
        Update::InlineQuery(query) => {
            println!("Query {}", query.text());

            // let doc = InputDocument::Document(InputStickeredMediaDocument { })

            let number: i32 = query.text().parse()?;
            let picture = render_raw_number(number).ok_or("idk")?;
            let mut cursor = std::io::Cursor::new(&picture);

            let file = client.upload_stream(&mut cursor, picture.len(), "sticker.webp".into()).await?;
            let uploaded = client.invoke(&UploadMedia {
                media: InputMediaUploadedDocument {
                    nosound_video: false,
                    file: file.input_file,
                    thumb: None,
                    ttl_seconds: None,
                    mime_type: "image/webp".into(),
                    attributes: vec!(),
                    spoiler: false,
                    force_file: false,
                    stickers: None,
                }.into(),
                peer: InputPeer::PeerSelf
            }).await?;

            if let MessageMedia::Document(MessageMediaDocument { document: Some(Document::Document(d)), .. }) = uploaded {
                let x = types::InputDocument {
                    id: d.id,
                    access_hash: d.access_hash,
                    file_reference: d.file_reference
                };

                query.answer(vec!(inline_query::InlineResult(enums::InputBotInlineResult::Document(InputBotInlineResultDocument {
                    document: InputDocument::Document(x),
                    title: Some("title".into()),
                    id: "1".into(),
                    r#type: "sticker".into(),
                    description: Some("asd".into()),
                    send_message: InputBotInlineMessageMediaAuto {
                        message: "".into(),
                        entities: None,
                        reply_markup: None
                    }.into() })))).send().await?;
            }

            // query.answer(vec!(inline_query::InlineResult(enums::InputBotInlineResult::Result(InputBotInlineResultDocument {
            //     title: Some("title".into()),
            //     id: "1".into(),
            //     r#type: "article".into(),
            //     description: Some("asd".into()),
            //     thumb: None,
            //     url: None,
            //     content: None,
            //     send_message: InputBotInlineMessageMediaAuto {
            //         message: "kek".into(),
            //         entities: None,
            //         no_webpage: true,
            //         reply_markup: None
            //     }.into() })))).send().await?;
            //     // send_message: InputBotInlineMessageText {
            //     //     message: "kek".into(),
            //     //     entities: None,
            //     //     no_webpage: true,
            //     //     reply_markup: None
            //     // }.into() })))).send().await?;
        }
        _ => {}
    }

    Ok(())
}

async fn async_main() -> Result {
    SimpleLogger::new()
        .with_level(log::LevelFilter::Debug)
        .init()
        .unwrap();

    let api_id = env!("TG_ID").parse().expect("TG_ID invalid");
    let api_hash = env!("TG_HASH").to_string();
    let token = env::args().skip(1).next().expect("token missing");

    println!("Connecting to Telegram...");
    let client = Client::connect(Config {
        session: Session::load_file_or_create(SESSION_FILE)?,
        api_id,
        api_hash: api_hash.clone(),
        params: InitParams {
            // Fetch the updates we missed while we were offline
            catch_up: true,
            ..Default::default()
        },
    })
        .await?;
    println!("Connected!");

    if !client.is_authorized().await? {
        println!("Signing in...");
        client.bot_sign_in(&token, api_id, &api_hash).await?;
        client.session().save_to_file(SESSION_FILE)?;
        println!("Signed in!");
    }

    println!("Waiting for messages...");

    // This code uses `select` on Ctrl+C to gracefully stop the client and have a chance to
    // save the session. You could have fancier logic to save the session if you wanted to
    // (or even save it on every update). Or you could also ignore Ctrl+C and just use
    // `while let Some(updates) =  client.next_updates().await?`.
    //
    // Using `tokio::select!` would be a lot cleaner but add a heavy dependency,
    // so a manual `select` is used instead by pinning async blocks by hand.
    loop {
        let update = {
            let exit = pin!(async { tokio::signal::ctrl_c().await });
            let upd = pin!(async { client.next_update().await });

            match select(exit, upd).await {
                Either::Left(_) => None,
                Either::Right((u, _)) => Some(u),
            }
        };

        let update = match update {
            None | Some(Ok(None)) => break,
            Some(u) => u?.unwrap(),
        };

        let handle = client.clone();
        task::spawn(async move {
            match handle_update(handle, update).await {
                Ok(_) => {}
                Err(e) => eprintln!("Error handling updates!: {}", e),
            }
        });
    }

    println!("Saving session file and exiting...");
    client.session().save_to_file(SESSION_FILE)?;
    Ok(())
}

fn main() -> Result {
    runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async_main())
}