mod bindings {
    use crate::DogFetcher;

    wit_bindgen::generate!({
        with: {
            "wasi:clocks/monotonic-clock@0.2.0": ::wasi::clocks::monotonic_clock,
            "wasi:http/incoming-handler@0.2.0": generate,
            "wasi:http/outgoing-handler@0.2.0": ::wasi::http::outgoing_handler,
            "wasi:http/types@0.2.0": ::wasi::http::types,
            "wasi:io/error@0.2.0": ::wasi::io::error,
            "wasi:io/poll@0.2.0": ::wasi::io::poll,
            "wasi:io/streams@0.2.0": ::wasi::io::streams,
        }
    });

    export!(DogFetcher);
}

use std::io::{Read as _, Write as _};

use bindings::exports::wasi::http::incoming_handler::Guest;
use wasi::http::types::*;

#[derive(serde::Deserialize)]
struct DogResponse {
    message: String,
}

struct DogFetcher;

impl Guest for DogFetcher {
    fn handle(_request: IncomingRequest, response_out: ResponseOutparam) {
        // Build a request to dog.ceo which returns a URL at which we can find a doggo
        let req = wasi::http::outgoing_handler::OutgoingRequest::new(Fields::new());
        req.set_scheme(Some(&Scheme::Https)).unwrap();
        req.set_authority(Some("dog.ceo")).unwrap();
        req.set_path_with_query(Some("/api/breeds/image/random"))
            .unwrap();

        // Perform the API call to dog.ceo, expecting a URL to come back as the response body
        let dog_picture_url = match wasi::http::outgoing_handler::handle(req, None) {
            Ok(resp) => {
                resp.subscribe().block();
                let response = resp
                    .get()
                    .expect("HTTP request response missing")
                    .expect("HTTP request response requested more than once")
                    .expect("HTTP request failed");
                if response.status() == 200 {
                    let response_body = response
                        .consume()
                        .expect("failed to get incoming request body");
                    let body = {
                        let mut buf = vec![];
                        let mut stream = response_body
                            .stream()
                            .expect("failed to get HTTP request response stream");
                        stream
                            .read_to_end(&mut buf)
                            .expect("failed to read value from HTTP request response stream");
                        buf
                    };
                    let _trailers = wasi::http::types::IncomingBody::finish(response_body);
                    let dog_response: DogResponse = serde_json::from_slice(&body).unwrap();
                    dog_response.message
                } else {
                    format!("HTTP request failed with status code {}", response.status())
                }
            }
            Err(e) => {
                format!("Got error when trying to fetch dog: {}", e)
            }
        };

        // Build the HTTP response we'll send back to the user
        let response = OutgoingResponse::new(Fields::new());
        response.set_status_code(200).unwrap();
        let response_body = response.body().unwrap();
        let mut write_stream = response_body.write().unwrap();

        ResponseOutparam::set(response_out, Ok(response));

        write_stream.write_all(dog_picture_url.as_bytes()).unwrap();
        drop(write_stream);

        OutgoingBody::finish(response_body, None).expect("failed to finish response body");
    }
}
