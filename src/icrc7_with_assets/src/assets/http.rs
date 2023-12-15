use std::{borrow::Cow, collections::HashMap};

use percent_encoding::percent_decode_str;
use regex::Regex;

use ic_canister_kit::http::MAX_RESPONSE_LENGTH;

use super::explore::explore;
use super::types::*;
use crate::assets_stable::State;
use ic_canister_kit::types::*;

#[candid::candid_method(query, rename = "http_request")]
fn __http_request(_req: CustomHttpRequest) -> CustomHttpResponse<'static> {
    todo!()
}

#[export_name = "canister_query http_request"]
fn http_request() {
    ic_cdk::setup();
    let req = ic_cdk::api::call::arg_data::<(CustomHttpRequest,)>().0;
    crate::assets_stable::with_state(|_state| _http_request(req, _state));
}

#[inline]
fn _http_request(req: CustomHttpRequest, state: &State) {
    let mut split_url = req.url.split('?');
    let request_headers = req.headers;

    let path = split_url.next().unwrap_or("/");
    let path = percent_decode_str(path).decode_utf8().unwrap();
    let params = split_url.next().unwrap_or("");
    let params = percent_decode_str(params).decode_utf8().unwrap();

    // ic_cdk::println!("============== path: {} -> {}", req.url, path);
    // for (key, value) in request_headers.iter() {
    //     ic_cdk::println!("header: {}: {}", key, value);
    // }

    let mut code = 200;
    let mut headers: HashMap<&str, Cow<str>> = HashMap::new();
    let body: Vec<u8>;
    let mut streaming_strategy: Option<StreamingStrategy> = None;

    if path == "/" {
        body = explore(&mut headers, state);
    } else {
        let file = state.assets.files.get(path.as_ref());
        if let Some(file) = file {
            let asset = state.assets.assets.get(&file.hash);
            if let Some(asset) = asset {
                let (_body, _streaming_strategy): (Vec<u8>, Option<StreamingStrategy>) = toast(
                    &path,
                    &params,
                    &request_headers,
                    file,
                    asset,
                    &mut code,
                    &mut headers,
                );
                body = _body;
                streaming_strategy = _streaming_strategy;
            } else {
                body = not_found(&mut code, &mut headers);
            }
        } else {
            body = not_found(&mut code, &mut headers);
        }
    }

    ic_cdk::api::call::reply((CustomHttpResponse {
        status_code: code,
        headers,
        body: body.into(),
        streaming_strategy,
    },));
}

fn toast<'a>(
    path: &str,
    params: &str,
    request_headers: &HashMap<String, String>,
    file: &'a AssetFile,
    asset: &AssetData,
    code: &mut u16,
    headers: &mut HashMap<&'a str, Cow<'a, str>>,
) -> (Vec<u8>, Option<StreamingStrategy>) {
    let (offset, offset_end, streaming_strategy) = set_headers(
        path,
        params,
        request_headers,
        file,
        asset.size as usize,
        code,
        headers,
    );

    (
        (&asset.data[offset..offset_end]).to_vec(),
        streaming_strategy,
    )
}

fn set_headers<'a>(
    path: &str,
    params: &str,
    request_headers: &HashMap<String, String>,
    file: &'a AssetFile,
    size: usize,
    code: &mut u16,
    headers: &mut HashMap<&'a str, Cow<'a, str>>,
) -> (usize, usize, Option<StreamingStrategy>) {
    // let mut gzip = false;
    // let mut content_type = "";
    // for (key, value) in file.headers.iter() {
    //     if &key.to_lowercase() == "content-type" {
    //         content_type = value;
    //     }
    //     if &key.to_lowercase() == "content-encoding" && value == "gzip" {
    //         gzip = true;
    //     }
    // }
    let reg = Regex::new(r"attachment=(.*\..*)?(&.*)?$").unwrap();
    for cap in reg.captures_iter(params) {
        let mut file_name = cap
            .get(1)
            .and_then(|m| Some(&params[m.start()..m.end()]))
            .unwrap_or("");
        if file_name.is_empty() {
            let mut s = file.path.split("/");
            while let Some(name) = s.next() {
                file_name = name;
            }
        }
        if !file_name.is_empty() {
            headers.insert(
                "Content-Disposition",
                format!("attachment; filename=\"{}\"", file_name).into(),
            ); // 下载文件名
        }
    }

    headers.insert("Accept-Ranges", "bytes".into());
    headers.insert("ETag", file.hash.to_string().into());

    headers.insert("Access-Control-Allow-Origin", "*".into());
    headers.insert(
        "Access-Control-Allow-Methods",
        "HEAD, GET, POST, OPTIONS".into(),
    );
    headers.insert(
        "Access-Control-Allow-Headers",
        "Origin,Access-Control-Request-Headers,Access-Control-Allow-Headers,DNT,X-Requested-With,X-Mx-ReqToken,Keep-Alive,X-Requested-With,If-Modified-Since,Cache-Control,Content-Type,Accept,Connection,Cook ie,X-XSRF-TOKEN,X-CSRF-TOKEN,Authorization".into(),
    );
    headers.insert(
        "Access-Control-Expose-Headers",
        "Accept-Ranges,Content-Length,Content-Range,Transfer-Encoding,Connection,Cache-Control,Content-Disposition"
            .into(),
    );
    headers.insert("Access-Control-Max-Age", "86400".into());

    // Range
    let mut start: usize = 0;
    let mut end: usize = size;
    if let Some(range) = {
        let mut range = None;
        for (key, value) in request_headers.iter() {
            if &key.to_lowercase() == "range" {
                range = Some(value.trim());
                break;
            }
        }
        range
    } {
        // bytes=start-end
        if range.starts_with("bytes=") {
            let range = &range[6..];
            let mut ranges = range.split("-");
            let s = ranges.next();
            let e = ranges.next();
            if let Some(s) = s {
                let s: usize = s.parse().unwrap_or(0);
                if s < size {
                    start = s
                };
            }
            if let Some(e) = e {
                let e: usize = e.parse().unwrap_or(size - 1);
                if start < e && e < size {
                    end = e + 1
                };
            }
        }
    }

    for (key, value) in file.headers.iter() {
        headers.insert(key, value.into());
    }

    let mut streaming_end = end;
    let mut streaming_strategy: Option<StreamingStrategy> = None;
    let range = streaming_end - start;
    if MAX_RESPONSE_LENGTH < range && start + MAX_RESPONSE_LENGTH < end {
        streaming_end = start + MAX_RESPONSE_LENGTH;
        streaming_strategy = Some(StreamingStrategy::Callback {
            callback: HttpRequestStreamingCallback::new(
                ic_cdk::id(),
                "http_request_streaming_callback".to_string(),
            ),
            token: StreamingCallbackToken {
                path: path.to_string(),
                params: params.to_string(),
                headers: request_headers.clone(),
                start: streaming_end as u64,
                end: end as u64,
            },
        });
        headers.insert("Transfer-Encoding", "chunked".into());
        headers.insert("Connection", "keep-alive".into());
    }
    // Content-Range: bytes 0-499/10000
    headers.insert(
        "Content-Range",
        format!("bytes {}-{}/{}", start, end - 1, size).into(),
    );
    // headers.insert("Content-Length", format!("{}", end - start).into());

    *code = 200;
    if end < size {
        *code = 206;
    }

    (start, streaming_end, streaming_strategy)
}

fn not_found<'a>(code: &mut u16, headers: &mut HashMap<&'a str, Cow<'a, str>>) -> Vec<u8> {
    *code = 404;

    headers.insert("Content-Type", "text/plain".into());

    b"Not found"[..].into()
}

#[ic_cdk::query]
fn http_request_streaming_callback(
    StreamingCallbackToken {
        path,
        params,
        headers,
        start,
        end,
    }: StreamingCallbackToken,
) -> StreamingCallbackHttpResponse {
    // ic_cdk::println!(
    //     "http_request_streaming_callback: {:?} {:?} {:?} {:?} {:?}",
    //     path,
    //     params,
    //     headers,
    //     start,
    //     end,
    // );
    if start == end {
        return StreamingCallbackHttpResponse {
            body: vec![],
            token: None,
        };
    }
    crate::assets_stable::with_state(|state| {
        let file = state.assets.files.get(&path);
        if let Some(file) = file {
            let asset = state.assets.assets.get(&file.hash);
            if let Some(asset) = asset {
                let start = start as usize;
                let end = end as usize;
                let mut streaming_end = end;
                let range = streaming_end - start;
                if MAX_RESPONSE_LENGTH < range && start + MAX_RESPONSE_LENGTH < end {
                    streaming_end = start + MAX_RESPONSE_LENGTH;
                }
                return StreamingCallbackHttpResponse {
                    body: (&asset.data[start..streaming_end]).to_vec(),
                    token: Some(StreamingCallbackToken {
                        path: path.to_string(),
                        params: params.to_string(),
                        headers: headers.clone(),
                        start: streaming_end as u64,
                        end: end as u64,
                    }),
                };
            }
        }
        StreamingCallbackHttpResponse {
            body: vec![],
            token: None,
        }
    })
}
