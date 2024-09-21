//! HTTP Server with JSON POST handler
//!
//! Go to 192.168.71.1 to test

use core::convert::TryInto;

use embedded_svc::{
    http::{Headers, Method},
    io::{Read, Write},
    wifi::{self, AccessPointConfiguration, AuthMethod},
};

use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    http::server::EspHttpServer,
    nvs::EspDefaultNvsPartition,
    wifi::{BlockingWifi, EspWifi},
};
use espcam::FrameBuffer;

use std::{fmt::Debug, time::Duration};
use esp_idf_svc::sys::nvs_flash_init;
use esp_idf_svc::sys::nvs_flash_erase;
use esp_idf_svc::sys::ESP_ERR_NVS_NO_FREE_PAGES;
use esp_idf_svc::sys::ESP_ERR_NVS_NEW_VERSION_FOUND;
use esp_idf_svc::sys::EspError;
use anyhow::{bail, Result};
use esp_idf_svc::{
    http::{server::Configuration},
};
use esp_idf_svc::hal::delay;

mod espcam;
pub use crate::espcam::Camera;
mod config;
pub use crate::config::get_config;
mod wifi_handler;
pub use crate::wifi_handler::my_wifi;

use log::*;

use serde::Deserialize;

const SSID: &str = "Iphone";
const PASSWORD: &str = "WIFI_PASS";
static INDEX_HTML: &str = include_str!("http_server_page.html");
static VIDEO_HTML: &str = include_str!("video_page.html");
// Max payload length
const MAX_LEN: usize = 128;

// Need lots of stack to parse JSON
const STACK_SIZE: usize = 10240;

const SESSION_TIMEOUT: Duration = Duration::new(240, 0);

// Wi-Fi channel, between 1 and 11
const CHANNEL: u8 = 11;

#[derive(Deserialize)]
struct FormData<'a> {
    first_name: &'a str,
    age: u32,
    birthplace: &'a str,
}

//Initializing NVS storage 
fn nvs_init() -> Result<(), EspError> {
    unsafe {
        let mut ret = nvs_flash_init();
        if ret == ESP_ERR_NVS_NO_FREE_PAGES as i32 || ret == ESP_ERR_NVS_NEW_VERSION_FOUND as i32 {
            //Ensure memory is empty before write
            nvs_flash_erase();
            ret = nvs_flash_init();
        }
        Ok(())
    }
}

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;
    
    let peripherals = Peripherals::take().unwrap();

    
    let _init = nvs_init();

    
    //Camera Pinout mapping. Use this config for esp32 Cam boards.
    //other boards have different pinouts check docs before running.
    let camera = Camera::new(
        peripherals.pins.gpio32,
        peripherals.pins.gpio0,
        peripherals.pins.gpio5,
        peripherals.pins.gpio18,
        peripherals.pins.gpio19,
        peripherals.pins.gpio21,
        peripherals.pins.gpio36,
        peripherals.pins.gpio39,
        peripherals.pins.gpio34,
        peripherals.pins.gpio35,
        peripherals.pins.gpio25,
        peripherals.pins.gpio23,
        peripherals.pins.gpio22,
        peripherals.pins.gpio26,
        peripherals.pins.gpio27,
        esp_idf_sys::camera::pixformat_t_PIXFORMAT_JPEG,
        esp_idf_sys::camera::framesize_t_FRAMESIZE_QVGA,
    )
    .unwrap();

    let wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, sys_loop.clone(), Some(nvs))?,
        sys_loop,
    )?;

    let mut server = create_server(wifi)?;

    //Main server function this handler get an image from the camera
    // and posts it to the /video handler below.
    server.fn_handler("/video/camera", Method::Get,move|request| {
        //Header set to allow for MJPEG streaming 
        let headers = [
        ("Content-Type", "multipart/x-mixed-replace; boundary=frame"),
        ];
            
        let mut response = request.into_response(200, Some("OK"), &headers).unwrap();
        loop{
            if let Some(framebuffer) = camera.get_framebuffer() {
                // Create a JPEG image
                let jpeg_data = framebuffer.data(); // Assuming this returns a JPEG frame
                let frame_header = format!("--frame\r\nContent-Type: image/jpeg\r\nContent-Length: {}\r\n\r\n", jpeg_data.len());
    
                // Send the frame header
                response.write_all(frame_header.as_bytes()).unwrap();
                // Send the JPEG image
                response.write_all(&jpeg_data).unwrap();
                response.write_all(b"\r\n").unwrap(); // End of frame
                
                info!("Picture Sent!");
                framebuffer.fb_return();

                /*
                set to 4 FPS. Higher FPS causes transmission errors.
                Higher FPS can be acheived either by reducing image 
                resolution (see espcam.rs) or by cooling the esp board.
                 */
                delay::Ets::delay_ms(250); 
            } else {
                // If no frame is available, you might want to handle it
                delay::Ets::delay_ms(100); // Avoid busy waiting
            }
            
        }
        
        Ok::<(), anyhow::Error>(())
    })?;

    //Main page receives video stream from /video/camera
    server.fn_handler("/video", Method::Get, |req| {
        req.into_ok_response()?
            .write_all(VIDEO_HTML.as_bytes())
            .map(|_| ())
    })?;
    
    server.fn_handler("/", Method::Get, |req| {
        req.into_ok_response()?
            .write_all(INDEX_HTML.as_bytes())
            .map(|_| ())
    })?;

    //Function kept from esprust example code to illistrate how to use POST reqs
    server.fn_handler::<anyhow::Error, _>("/post", Method::Post, |mut req| {
        let len = req.content_len().unwrap_or(0) as usize;

        if len > MAX_LEN {
            req.into_status_response(413)?
                .write_all("Request too big".as_bytes())?;
            return Ok(());
        }

        let mut buf = vec![0; len];
        req.read_exact(&mut buf)?;
        let mut resp = req.into_ok_response()?;

        if let Ok(form) = serde_json::from_slice::<FormData>(&buf) {
            write!(
                resp,
                "Hello, {}-year-old {} from {}!",
                form.age, form.first_name, form.birthplace
            )?;
        } else {
            resp.write_all("JSON error".as_bytes())?;
        }

        Ok(())
    })?;
    // Keep server running beyond when main() returns (forever)
    // Do not call this if you ever want to stop or access it later.
    // Otherwise you can either add an infinite loop so the main task
    // never returns, or you can move it to another thread.
    // https://doc.rust-lang.org/stable/core/mem/fn.forget.html
    core::mem::forget(server);

    // Main task no longer needed, free up some memory
    Ok(())
}


/*
Function used to setup and create a wifi server.
*/
fn create_server(mut wifi: BlockingWifi<EspWifi>) -> anyhow::Result<EspHttpServer<'static>> {

    let wifi_configuration = wifi::Configuration::AccessPoint(AccessPointConfiguration {
        ssid: SSID.try_into().unwrap(),
        ssid_hidden: false,
        auth_method: AuthMethod::WPA2Personal,
        password: PASSWORD.try_into().unwrap(),
        channel: CHANNEL,
        ..Default::default()
    });
    wifi.set_configuration(&wifi_configuration)?;
    wifi.start()?;
    wifi.wait_netif_up()?;

    info!(
        "Created Wi-Fi with WIFI_SSID `{}` and WIFI_PASS `{}`",
        SSID, PASSWORD
    );

    let server_configuration = esp_idf_svc::http::server::Configuration {
        stack_size: STACK_SIZE,
        session_timeout: SESSION_TIMEOUT,
        ..Default::default()
    };

    // Keep wifi running beyond when this function returns (forever)
    // Do not call this if you ever want to stop or access it later.
    // Otherwise it should be returned from this function and kept somewhere
    // so it does not go out of scope.
    // https://doc.rust-lang.org/stable/core/mem/fn.forget.html
    core::mem::forget(wifi);

    Ok(EspHttpServer::new(&server_configuration)?)
}
