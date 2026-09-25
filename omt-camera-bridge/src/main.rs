//! Reads raw YUV420 (I420) camera frames from c2/socketbridge.py over a Unix
//! socket, converts them to UYVY, and streams them out over Open Media
//! Transport (OMT) so they show up as a source in OBS/vMix/etc.
//!
//! Usage:
//!   omt-camera-bridge [--socket PATH] [--name NAME] [--width W] [--height H] [--fps N]

use std::io::Read;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::thread;
use std::time::{Duration, Instant};

use openmediatransport::{Codec, ColorSpace, Discovery, FrameType, MediaFrame, Sender};
use yuv::{BufferStoreMut, YuvPackedImageMut, YuvPlanarImage, yuv420_to_uyvy422};

struct Args {
    socket_path: String,
    name: String,
    width: i32,
    height: i32,
    fps_n: i32,
    fps_d: i32,
}

impl Args {
    fn parse() -> Self {
        let mut a = Args {
            socket_path: "/run/c2-video.sock".to_string(),
            name: "C2 Camera".to_string(),
            width: 1920,
            height: 1080,
            fps_n: 30,
            fps_d: 1,
        };
        let mut it = std::env::args().skip(1);
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--socket" => a.socket_path = it.next().expect("--socket needs a value"),
                "--name" => a.name = it.next().expect("--name needs a value"),
                "--width" => {
                    a.width = it
                        .next()
                        .expect("--width needs a value")
                        .parse()
                        .expect("bad --width")
                }
                "--height" => {
                    a.height = it
                        .next()
                        .expect("--height needs a value")
                        .parse()
                        .expect("bad --height")
                }
                "--fps" => {
                    a.fps_n = it
                        .next()
                        .expect("--fps needs a value")
                        .parse()
                        .expect("bad --fps")
                }
                other => eprintln!("omt-camera-bridge: ignoring unknown argument {other:?}"),
            }
        }
        a
    }
}

/// Assumes a tightly-packed I420 buffer (stride == width for Y, width/2 for
/// U/V) and an even width - what picamera2's "YUV420" format hands off.
fn i420_to_uyvy(i420: &[u8], width: usize, height: usize) -> Vec<u8> {
    let y_size = width * height;
    let c_w = width / 2;
    let c_h = height / 2;

    let planar = YuvPlanarImage {
        y_plane: &i420[..y_size],
        y_stride: width as u32,
        u_plane: &i420[y_size..y_size + c_w * c_h],
        u_stride: c_w as u32,
        v_plane: &i420[y_size + c_w * c_h..y_size + 2 * c_w * c_h],
        v_stride: c_w as u32,
        width: width as u32,
        height: height as u32,
    };

    let mut out = vec![0u8; width * height * 2];
    let mut packed = YuvPackedImageMut {
        yuy: BufferStoreMut::Borrowed(&mut out),
        yuy_stride: (width * 2) as u32,
        width: width as u32,
        height: height as u32,
    };

    yuv420_to_uyvy422(&mut packed, &planar).expect("yuv420_to_uyvy422");
    out
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    if args.width % 2 != 0 {
        return Err("--width must be even (UYVY is 4:2:2)".into());
    }

    let mut sender = Sender::create(&args.name, FrameType::VIDEO | FrameType::METADATA)?;
    let port = sender.port();
    let mut discovery = Discovery::new()?;
    discovery.register(&args.name, port)?;
    println!(
        "omt-camera-bridge: sending {:?} on port {port} ({}x{} @ {}/{} fps), reading I420 from {}",
        args.name, args.width, args.height, args.fps_n, args.fps_d, args.socket_path
    );

    let width = args.width as usize;
    let height = args.height as usize;
    let i420_size = width * height * 3 / 2;
    let mut i420_buf = vec![0u8; i420_size];

    let epoch = Instant::now();
    let mut last_sub = false;

    // Wall-clock, not core-time: `yuv` parallelizes internally, so a low
    // percentage here doesn't mean low total CPU - check top/htop for that.
    let frame_budget = Duration::from_secs_f64(1.0 / args.fps_n.max(1) as f64);
    let mut conversion_time = Duration::ZERO;
    let mut converted_frames: u64 = 0;
    let mut stats_since = Instant::now();

    loop {
        // Retry until the Python side is up, or comes back after a restart.
        let mut stream = loop {
            match UnixStream::connect(&args.socket_path) {
                Ok(s) => break s,
                Err(e) => {
                    eprintln!("omt-camera-bridge: waiting for {}: {e}", args.socket_path);
                    thread::sleep(Duration::from_secs(1));
                }
            }
        };
        println!("omt-camera-bridge: connected to {}", args.socket_path);

        loop {
            if stream.read_exact(&mut i420_buf).is_err() {
                eprintln!(
                    "omt-camera-bridge: lost connection to {}, reconnecting",
                    args.socket_path
                );
                let _ = stream.shutdown(Shutdown::Both);
                break;
            }

            sender.poll_accept()?;
            sender.poll_peer_metadata()?;

            let subscribed = sender.video_subscribed();
            if subscribed != last_sub {
                println!("omt-camera-bridge: video subscribed: {subscribed}");
                last_sub = subscribed;
            }
            if !subscribed {
                continue; // still drain the socket above so Python never blocks
            }

            let conv_start = Instant::now();
            let data = i420_to_uyvy(&i420_buf, width, height);
            conversion_time += conv_start.elapsed();
            converted_frames += 1;

            let frame = MediaFrame {
                frame_type: FrameType::VIDEO,
                timestamp: (epoch.elapsed().as_micros() as i64) * 10, // 100ns units
                codec: Codec::Uyvy as i32,
                width: args.width,
                height: args.height,
                stride: args.width * 2,
                frame_rate_n: args.fps_n,
                frame_rate_d: args.fps_d,
                aspect_ratio: args.width as f32 / args.height.max(1) as f32,
                color_space: ColorSpace::Bt709,
                data,
                ..Default::default()
            };
            sender.send_video(frame)?;

            if stats_since.elapsed() >= Duration::from_secs(5) {
                let avg = conversion_time / converted_frames.max(1) as u32;
                let pct_of_frame_budget =
                    avg.as_secs_f64() / frame_budget.as_secs_f64() * 100.0;
                println!(
                    "omt-camera-bridge: conversion avg {:.2}ms/frame ({:.1}% of one frame's time budget @ {} fps) over {} frames",
                    avg.as_secs_f64() * 1000.0,
                    pct_of_frame_budget,
                    args.fps_n,
                    converted_frames
                );
                conversion_time = Duration::ZERO;
                converted_frames = 0;
                stats_since = Instant::now();
            }
        }
    }
}
