use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use dotenv::dotenv;
use hound;
use reqwest::multipart::{Form, Part};
use reqwest::Client;
use rodio::{Decoder, OutputStream, Sink};
use serde_json::json;
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use std::{env, fs};
use tokio::runtime::Runtime;
#[derive(serde::Deserialize)]
struct Translation {
    text: String,
}

async fn play_text(text: &String) {
    let openai_key = match env::var("OPENAI_KEY") {
        Ok(key) => key,
        Err(_) => panic!("No openai key found"),
    };
    let client = reqwest::Client::new();
    let res = client
        .post("https://api.openai.com/v1/audio/speech")
        .header("Content-Type", "application/json")
        .bearer_auth(openai_key)
        .json(&json!({
            "model": "tts-1",
            "input": text,
            "voice": "alloy"
        }))
        .send()
        .await
        .expect("Unable to send request");

    let audio = res.bytes().await.expect("Unable to read response as bytes");

    let cursor = Cursor::new(audio);

    let (_stream, stream_handle) = OutputStream::try_default().unwrap();
    let sink = Sink::try_new(&stream_handle).unwrap();
    let source = Decoder::new_mp3(cursor).unwrap();
    sink.append(source);
    sink.sleep_until_end();
}

fn main() {
    dotenv().ok();
    let (tx, rx) = mpsc::channel();
    let host = cpal::default_host();
    let input_device = host.default_input_device().unwrap();
    let input_format = input_device.default_input_config().unwrap();
    let config = cpal::StreamConfig {
        channels: input_format.channels(),
        sample_rate: input_format.sample_rate(),
        buffer_size: cpal::BufferSize::Default,
    };

    let spec = hound::WavSpec {
        channels: config.channels as u16,
        sample_rate: config.sample_rate.0,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

    let writer = Arc::new(Mutex::new(
        hound::WavWriter::create("src/files/output0.wav", spec).unwrap(),
    ));
    let start_time = Arc::new(Mutex::new(Instant::now()));
    let file_count = Arc::new(Mutex::new(0));

    let stream = input_device
        .build_input_stream(
            &config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                let tx = tx.clone();

                let mut writer = writer.lock().unwrap();
                let mut start_time = start_time.lock().unwrap();
                let mut file_count = file_count.lock().unwrap();

                for &sample in data {
                    writer.write_sample(sample).unwrap();
                }

                if start_time.elapsed() >= Duration::from_secs(5) {
                    let file_name = format!("src/files/output{}.wav", *file_count);
                    let new_file_name = format!("src/files/output{}.wav", *file_count + 1);
                    *writer = hound::WavWriter::create(&new_file_name, spec).unwrap();
                    *file_count += 1;
                    *start_time = Instant::now();
                    tx.send(file_name.clone()).unwrap();
                }
            },
            move |err| {
                eprintln!("Error listening to this fuck: {}", err);
            },
            None,
        )
        .unwrap();

    stream.play().unwrap();
    println!("Start talking");

    let rt = Runtime::new().unwrap();

    rt.block_on(async {
        while let Ok(file_name) = rx.recv() {
            let translated_text = translate_file(&file_name).await;

            tokio::spawn(async move {
                play_text(&translated_text).await;
            });
            fs::remove_file(&file_name).expect("Couldn't remove file ");
        }
    });
}

async fn translate_file(file_path: &String) -> String {
    let openai_key = match env::var("OPENAI_KEY") {
        Ok(key) => key,
        Err(_) => panic!("No openai key found"),
    };
    let client = Client::new();

    let path = Path::new(file_path);
    let mut file = File::open(&path).unwrap();
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer).unwrap();
    let file_name = path.file_name().unwrap().to_str().unwrap().to_owned();
    let part = Part::bytes(buffer).file_name(file_name);

    let form = Form::new().part("file", part).text("model", "whisper-1");

    let res = client
        .post("https://api.openai.com/v1/audio/translations")
        .bearer_auth(openai_key)
        .multipart(form)
        .send()
        .await
        .unwrap();

    let text = res.text().await.unwrap();
    let data: Translation = serde_json::from_str(&text).unwrap();
    if data.text.contains("finish") {
        let dir_path = "src/files";

        let files = fs::read_dir(dir_path)
            .expect("Failed to read directory")
            .map(|entry| entry.expect("Failed to get directory entry").path())
            .collect::<Vec<_>>();

        for file in files {
            fs::remove_file(file).expect("Failed to remove file");
        }

        std::process::exit(0);
    }
    data.text
}
