use std::io::Cursor;

use actix_web::http::header::LOCATION;
use actix_web::{get, web, HttpResponse};
use image::{ImageBuffer, Rgba};

use log::{debug, error, warn};
use serde::Deserialize;
use sqlx::Error::RowNotFound;
use sqlx::PgPool;
use tokio::time::Instant;
use unicode_truncate::UnicodeTruncateStr;

use crate::maps::overlay_map::OverlayMapTask;
use crate::maps::overlay_text::{OverlayText, CANTARELL_BOLD, CANTARELL_REGULAR};
use crate::models::Location;
use crate::models::LocationKeyAlias;
use crate::utils;

mod fetch_tile;
mod overlay_map;
mod overlay_text;

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(maps_handler);
    let tile_cache = std::env::temp_dir().join("tiles");
    if !tile_cache.exists() {
        std::fs::create_dir(tile_cache).unwrap();
    }
}

async fn get_localised_data(
    conn: &PgPool,
    id: &str,
    should_use_english: bool,
) -> Result<Location, HttpResponse> {
    let result = if should_use_english {
        sqlx::query_as!(Location, "SELECT name,last_calendar_scrape_at,calendar_url,type,type_common_name,lat,lon FROM en WHERE key = $1", id)
            .fetch_all(conn)
            .await
    } else {
        sqlx::query_as!(Location, "SELECT name,last_calendar_scrape_at,calendar_url,type,type_common_name,lat,lon FROM de WHERE key = $1", id)
            .fetch_all(conn)
            .await
    };

    match result {
        Ok(r) => match r.len() {
            0 => Err(HttpResponse::NotFound()
                .content_type("text/plain")
                .body("Not found")),
            _ => Ok(r[0].clone()),
        },
        Err(e) => {
            error!("Error preparing statement: {e:?}");
            return Err(HttpResponse::InternalServerError()
                .content_type("text/plain")
                .body("Internal Server Error"));
        }
    }
}

async fn construct_image_from_data(data: Location, format: PreviewFormat) -> Option<Vec<u8>> {
    let start_time = Instant::now();
    let mut img = match format {
        PreviewFormat::OpenGraph => image::RgbaImage::new(1200, 630),
        PreviewFormat::Square => image::RgbaImage::new(1200, 1200),
    };

    // add the map
    if !OverlayMapTask::with(&data).draw_onto(&mut img).await {
        return None;
    }
    debug!("map draw {:?}", start_time.elapsed());
    draw_pin(&mut img);

    draw_bottom(&data, &mut img);
    debug!("overlay finish {:?}", start_time.elapsed());
    Some(wrap_image_in_response(&img))
}

/// add the location pin image to the center
fn draw_pin(img: &mut ImageBuffer<Rgba<u8>, Vec<u8>>) {
    let pin = image::load_from_memory(include_bytes!("static/pin.png")).unwrap();
    image::imageops::overlay(
        img,
        &pin,
        (img.width() as i64) / 2 - i64::from(pin.width()) / 2,
        ((img.height() as i64) - 125) / 2 - i64::from(pin.height()),
    );
}

fn wrap_image_in_response(img: &image::RgbaImage) -> Vec<u8> {
    let mut w = Cursor::new(Vec::new());
    img.write_to(&mut w, image::ImageFormat::Png).unwrap();
    w.into_inner()
}
const WHITE_PIXEL: Rgba<u8> = Rgba([255, 255, 255, 255]);
fn draw_bottom(data: &Location, img: &mut image::RgbaImage) {
    // draw background white
    for x in 0..img.width() {
        for y in img.height() - 125..img.height() {
            img.put_pixel(x, y, WHITE_PIXEL);
        }
    }
    // add our logo so the bottom
    let logo = image::load_from_memory(include_bytes!("static/logo.png")).unwrap();
    image::imageops::overlay(
        img,
        &logo,
        15,
        img.height() as i64 - (125 / 2) - (i64::from(logo.height()) / 2) + 9,
    );
    let name = if data.name.chars().count() >= 45 {
        format!("{}...", data.name.unicode_truncate(45).0)
    } else {
        data.name.clone()
    };
    OverlayText::with(&name, &CANTARELL_BOLD)
        .at(10, 125 - 10)
        .draw_onto(img);
    OverlayText::with(&data.type_common_name, &CANTARELL_REGULAR)
        .at(10, 125 - 50)
        .draw_onto(img);
}

fn load_default_image() -> Vec<u8> {
    warn!("Loading default preview image, as map rendering failed. Check the connection to the tileserver");
    let img = image::load_from_memory(include_bytes!("static/logo-card.png")).unwrap();
    // encode the image as PNG
    let mut w = Cursor::new(Vec::new());
    img.write_to(&mut w, image::ImageFormat::Png).unwrap();
    w.into_inner()
}

async fn get_possible_redirect_url(conn: &PgPool, query: &str, args: &QueryArgs) -> Option<String> {
    let result = sqlx::query_as!(
        LocationKeyAlias,
        r#"
        SELECT key, visible_id, type
        FROM aliases
        WHERE alias = $1 AND key <> alias
        LIMIT 1"#,
        query
    )
    .fetch_one(conn)
    .await;
    match result {
        Ok(d) => Some(format!(
            "https://nav.tum.de/api/preview/{key}?lang={lang}&format={format}",
            key = d.key,
            lang = args.lang.serialise(),
            format = args.format.serialise()
        )),
        Err(RowNotFound) => None,
        Err(e) => {
            error!("Error requesting alias for {query}: {e:?}");
            None
        }
    }
}

#[derive(Deserialize, Default, Debug, Copy, Clone)]
#[serde(rename_all = "snake_case")]
enum PreviewFormat {
    #[default]
    OpenGraph,
    Square,
}
impl PreviewFormat {
    fn serialise(&self) -> String {
        match self {
            PreviewFormat::OpenGraph => "open_graph".to_string(),
            PreviewFormat::Square => "square".to_string(),
        }
    }
}

#[derive(Deserialize, Default, Debug)]
#[serde(rename_all = "snake_case")]
#[serde(default)]
struct QueryArgs {
    #[serde(flatten)]
    lang: utils::LangQueryArgs,
    format: PreviewFormat,
}

#[get("/{id}")]
pub async fn maps_handler(
    params: web::Path<String>,
    web::Query(args): web::Query<QueryArgs>,
    data: web::Data<crate::AppData>,
) -> HttpResponse {
    let start_time = Instant::now();
    let id = params
        .into_inner()
        .replace(|c: char| c.is_whitespace() || c.is_control(), "");
    if let Some(redirect_url) = get_possible_redirect_url(&data.db, &id, &args).await {
        let mut res = HttpResponse::PermanentRedirect();
        res.insert_header((LOCATION, redirect_url));
        return res.finish();
    }
    let data = match get_localised_data(&data.db, &id, args.lang.should_use_english()).await {
        Ok(data) => data,
        Err(e) => {
            return e;
        }
    };
    let img = construct_image_from_data(data, args.format)
        .await
        .unwrap_or_else(load_default_image);

    debug!(
        "Preview Generation for {id} took {elapsed:?}",
        elapsed = start_time.elapsed()
    );
    HttpResponse::Ok().content_type("image/png").body(img)
}
