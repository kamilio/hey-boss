//! SDK transport uses a local CLI subprocess, never a service or network socket.
use crate::{Error, MAX_SECRET, Record, Result, Secret, Store, backend};
use base64::{Engine, engine::general_purpose::STANDARD as B64};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, process::Command};
use zeroize::Zeroizing;

#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Get {
        name: String,
    },
    Put {
        name: String,
        data: String,
        kind: String,
        expected: Option<String>,
    },
}
#[derive(Serialize, Deserialize)]
struct Response {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Error>,
    #[serde(skip_serializing_if = "Option::is_none")]
    record: Option<Record>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<String>,
}
fn execute(store: &mut Store, request: Request) -> Result<Response> {
    match request {
        Request::Get { name } => {
            let (record, secret) = store.read(&name)?;
            Ok(Response {
                ok: true,
                error: None,
                record: Some(record),
                data: Some(B64.encode(secret.expose())),
            })
        }
        Request::Put {
            name,
            data,
            kind,
            expected,
        } => {
            let data = Zeroizing::new(data);
            let secret = Secret::new(
                B64.decode(data.as_bytes())
                    .map_err(|_| Error::InvalidInput)?,
            );
            let record = store.put(&name, secret.expose(), &kind, expected.as_deref())?;
            Ok(Response {
                ok: true,
                error: None,
                record: Some(record),
                data: None,
            })
        }
    }
}
/// Only use on private pipes. Neither requests nor responses belong in logs.
pub fn serve(store: &mut Store, bytes: &[u8]) -> Secret {
    let result = if bytes.len() > 2 * MAX_SECRET {
        Err(Error::TooLarge)
    } else {
        serde_json::from_slice(bytes)
            .map_err(|_| Error::InvalidInput)
            .and_then(|req| execute(store, req))
    };
    let mut response = result.unwrap_or_else(|error| Response {
        ok: false,
        error: Some(error),
        record: None,
        data: None,
    });
    let bytes = serde_json::to_vec(&response).expect("fixed wire response");
    if let Some(mut data) = response.data.take() {
        zeroize::Zeroize::zeroize(&mut data);
    }
    Secret::new(bytes)
}

/// Defaults to the installed `hey-boss` on PATH. Works while its daemon is stopped.
pub struct Client {
    binary: PathBuf,
}
impl Default for Client {
    fn default() -> Self {
        Self {
            binary: "hey-boss".into(),
        }
    }
}
impl Client {
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
        }
    }
    fn call(&self, mut request: Request) -> Result<Response> {
        let input = Secret::new(serde_json::to_vec(&request).map_err(|_| Error::InvalidInput)?);
        if let Request::Put { data, .. } = &mut request {
            zeroize::Zeroize::zeroize(data);
        }
        let mut cmd = Command::new(&self.binary);
        cmd.args(["secrets", "rpc"]);
        let output = backend::command(cmd, input.expose())?;
        let response: Response =
            serde_json::from_slice(output.expose()).map_err(|_| Error::StoreUnavailable)?;
        if !response.ok {
            return Err(response.error.unwrap_or(Error::StoreUnavailable));
        }
        Ok(response)
    }
    pub fn get(&self, name: &str) -> Result<(Record, Secret)> {
        let response = self.call(Request::Get { name: name.into() })?;
        let data = Zeroizing::new(response.data.ok_or(Error::InvalidRecord)?);
        let bytes = B64
            .decode(data.as_bytes())
            .map_err(|_| Error::InvalidRecord)?;
        Ok((
            response.record.ok_or(Error::InvalidRecord)?,
            Secret::new(bytes),
        ))
    }
    pub fn put(
        &self,
        name: &str,
        bytes: &[u8],
        kind: &str,
        expected: Option<&str>,
    ) -> Result<Record> {
        if bytes.len() > MAX_SECRET {
            return Err(Error::TooLarge);
        }
        self.call(Request::Put {
            name: name.into(),
            data: B64.encode(bytes),
            kind: kind.into(),
            expected: expected.map(str::to_owned),
        })?
        .record
        .ok_or(Error::InvalidRecord)
    }
}
