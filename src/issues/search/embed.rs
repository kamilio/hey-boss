use super::index::{DIMENSIONS, MODEL};
use crate::issues::{Error, Result};
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(super) fn request(input: &[String], timeout: Duration) -> Result<Vec<Vec<f32>>> {
    let executable = std::env::current_exe()?.with_file_name("hey-proxy");
    let executable = if executable.exists() {
        executable
    } else {
        "hey-proxy".into()
    };
    let mut child = Command::new(executable)
        .args(["embed", "--timeout-ms", &timeout.as_millis().to_string()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| Error::new("search_proxy", "Embedding proxy unavailable"))?;
    let started = Instant::now();
    let payload = serde_json::to_vec(
        &serde_json::json!({"model":MODEL,"dimensions":DIMENSIONS,"input":input}),
    )?;
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || stdin.write_all(&payload));
    let stdout = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let success = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s.success(),
            Ok(None) if started.elapsed() < timeout + Duration::from_millis(100) => {
                std::thread::sleep(Duration::from_millis(10))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let _ = writer.join();
    let output = reader
        .join()
        .ok()
        .and_then(std::result::Result::ok)
        .filter(|v| v.len() <= 4 * 1024 * 1024);
    if !success {
        return Err(Error::new(
            "search_proxy",
            "Semantic search is temporarily unavailable",
        ));
    }
    let output = output.ok_or_else(|| Error::new("search_proxy", "Invalid embedding response"))?;
    decode(&output, input.len())
}

fn decode(output: &[u8], count: usize) -> Result<Vec<Vec<f32>>> {
    let response: serde_json::Value = serde_json::from_slice(output)
        .map_err(|_| Error::new("search_proxy", "Invalid embedding response"))?;
    let data = response["data"]
        .as_array()
        .filter(|a| a.len() == count)
        .ok_or_else(|| Error::new("search_proxy", "Invalid embedding count"))?;
    let mut vectors = vec![None; count];
    for row in data {
        let index = row["index"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| *n < count)
            .ok_or_else(|| Error::new("search_proxy", "Invalid embedding index"))?;
        let mut vector: Vec<f32> = serde_json::from_value(row["embedding"].clone())
            .map_err(|_| Error::new("search_proxy", "Invalid embedding vector"))?;
        let length = vector.iter().map(|n| n * n).sum::<f32>().sqrt();
        if vector.len() != DIMENSIONS
            || !length.is_finite()
            || length <= 0.0
            || vectors[index].is_some()
        {
            return Err(Error::new("search_proxy", "Invalid embedding dimensions"));
        }
        for value in &mut vector {
            *value /= length;
        }
        vectors[index] = Some(vector);
    }
    vectors
        .into_iter()
        .map(|v| v.ok_or_else(|| Error::new("search_proxy", "Missing embedding vector")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row(index: usize, value: f32) -> serde_json::Value {
        serde_json::json!({"index":index,"embedding":vec![value;DIMENSIONS]})
    }
    #[test]
    fn provider_vectors_are_reordered_normalized_and_validated() {
        let data =
            serde_json::to_vec(&serde_json::json!({"data":[row(1,2.0),row(0,1.0)]})).unwrap();
        let vectors = decode(&data, 2).unwrap();
        assert!((vectors[0].iter().map(|n| n * n).sum::<f32>() - 1.0).abs() < 0.0001);
        for rows in [
            vec![row(0, 0.0)],
            vec![row(1, 1.0)],
            vec![serde_json::json!({"index":0,"embedding":[1.0]})],
            vec![row(0, 1.0), row(0, 2.0)],
        ] {
            let count = rows.len();
            let data = serde_json::to_vec(&serde_json::json!({"data":rows})).unwrap();
            assert!(decode(&data, count).is_err());
        }
        assert!(decode(b"not json", 1).is_err());
    }
}
