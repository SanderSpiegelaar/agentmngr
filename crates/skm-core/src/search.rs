use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::Error;

const DEFAULT_API: &str = "https://skills.sh";
const CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub source: String,
    pub skill_id: String,
    pub name: String,
    pub installs: u64,
}

impl SearchResult {
    pub fn install_source(&self) -> String {
        format!("{}@{}", self.source, self.skill_id)
    }
}

#[derive(Deserialize)]
struct SearchResponse {
    skills: Vec<SearchResult>,
}

pub struct SearchClient {
    base_url: String,
    agent: ureq::Agent,
    cache: HashMap<(String, usize), (Instant, Vec<SearchResult>)>,
}

impl SearchClient {
    pub fn from_env() -> Self {
        let primary = std::env::var("SKILLSMGR_SKILLS_API").ok();
        let fallback = std::env::var("SKILLS_API_URL").ok();
        Self::new(api_url(primary.as_deref(), fallback.as_deref()))
    }

    pub fn new(base_url: &str) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(REQUEST_TIMEOUT))
            .timeout_resolve(Some(Duration::from_secs(1)))
            .timeout_connect(Some(Duration::from_secs(1)))
            .build()
            .into();
        Self {
            base_url: base_url.trim_end_matches('/').to_owned(),
            agent,
            cache: HashMap::new(),
        }
    }

    /// Blocking I/O; call from a worker, not the UI thread.
    pub fn search(&mut self, query: &str, limit: usize) -> Result<Vec<SearchResult>, Error> {
        self.cache.retain(|_, (at, _)| at.elapsed() < CACHE_TTL);
        let key = (query.to_owned(), limit);
        if let Some((_, results)) = self.cache.get(&key) {
            return Ok(results.clone());
        }

        let mut response = self
            .agent
            .get(format!("{}/api/search", self.base_url))
            .query("q", query)
            .query("limit", limit.to_string())
            .call()
            .map_err(|error| Error::Network(format!("skills.sh search failed: {error}")))?;
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|error| Error::Network(format!("skills.sh response failed: {error}")))?;
        let mut response: SearchResponse = serde_json::from_str(&body)
            .map_err(|error| Error::Network(format!("Invalid skills.sh response: {error}")))?;
        response.skills.retain(|result| {
            result.source.split_once('/').is_some_and(|(owner, repo)| {
                !owner.is_empty() && !repo.is_empty() && !repo.contains('/')
            }) && !result.skill_id.is_empty()
        });
        self.cache
            .insert(key, (Instant::now(), response.skills.clone()));
        Ok(response.skills)
    }
}

fn api_url<'a>(primary: Option<&'a str>, fallback: Option<&'a str>) -> &'a str {
    primary
        .filter(|url| !url.is_empty())
        .or_else(|| fallback.filter(|url| !url.is_empty()))
        .unwrap_or(DEFAULT_API)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use super::*;

    const RESULTS: &str = r#"{"skills":[{"id":"acme/tools/review","source":"acme/tools","skillId":"review","name":"Code review","installs":123}]}"#;

    fn serve(
        responses: Vec<(&'static str, &'static str)>,
    ) -> (String, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let server = thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body) in responses {
                let deadline = Instant::now() + Duration::from_secs(10);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline, "missing HTTP request");
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("{error}"),
                    }
                };
                // macOS accepted sockets inherit the listener's nonblocking mode.
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    stream.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                }
                requests.push(String::from_utf8(request).unwrap());
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
            requests
        });
        (url, server)
    }

    #[test]
    fn search_encodes_query_and_maps_install_source() {
        let (url, server) = serve(vec![("200 OK", RESULTS)]);
        let results = SearchClient::new(&format!("{url}/"))
            .search("rust & café", 7)
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].install_source(), "acme/tools@review");
        assert_eq!(results[0].name, "Code review");
        assert_eq!(results[0].installs, 123);
        let requests = server.join().unwrap();
        assert!(
            requests[0]
                .starts_with("GET /api/search?q=rust%20%26%20caf%C3%A9&limit=7 HTTP/1.1\r\n")
        );
    }

    #[test]
    fn search_omits_results_without_repository_install_sources() {
        let (url, server) = serve(vec![(
            "200 OK",
            r#"{"skills":[
                {"source":"open.feishu.cn","skillId":"lark-event","name":"Lark","installs":1},
                {"source":"owner/","skillId":"empty-repo","name":"Empty","installs":1},
                {"source":"owner/repo/path","skillId":"nested","name":"Nested","installs":1},
                {"source":"owner/repo","skillId":"","name":"Missing ID","installs":1},
                {"source":"acme/tools","skillId":"review","name":"Review","installs":2}
            ]}"#,
        )]);
        let results = SearchClient::new(&url).search("review", 50).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].install_source(), "acme/tools@review");
        server.join().unwrap();
    }

    #[test]
    fn cached_search_works_offline() {
        for body in [RESULTS, r#"{"skills":[]}"#] {
            let (url, server) = serve(vec![("200 OK", body)]);
            let mut client = SearchClient::new(&url);
            let expected = client.search("review", 50).unwrap();
            server.join().unwrap();
            assert_eq!(client.search("review", 50).unwrap(), expected);
            assert_eq!(client.search("other", 50).unwrap_err().code(), "E_NETWORK");
            assert_eq!(client.search("review", 1).unwrap_err().code(), "E_NETWORK");
        }
    }

    #[test]
    fn cache_expires_after_ten_minutes() {
        let (url, server) = serve(vec![("200 OK", RESULTS), ("200 OK", r#"{"skills":[]}"#)]);
        let mut client = SearchClient::new(&url);
        assert_eq!(client.search("review", 50).unwrap().len(), 1);
        client.cache.get_mut(&("review".into(), 50)).unwrap().0 =
            Instant::now() - Duration::from_secs(600);
        assert!(client.search("review", 50).unwrap().is_empty());
        assert_eq!(server.join().unwrap().len(), 2);
    }

    #[test]
    fn offline_returns_network_error_promptly() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = SearchClient::new(&format!("http://{}", listener.local_addr().unwrap()));
        drop(listener);
        let start = Instant::now();
        assert_eq!(client.search("review", 50).unwrap_err().code(), "E_NETWORK");
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn stalled_server_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut client = SearchClient::new(&format!("http://{}", listener.local_addr().unwrap()));
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut buffer = [0; 1024];
            while stream.read(&mut buffer).unwrap_or(0) > 0 {}
        });
        let start = Instant::now();
        assert_eq!(client.search("review", 50).unwrap_err().code(), "E_NETWORK");
        assert!(start.elapsed() < Duration::from_secs(5));
        server.join().unwrap();
    }

    #[test]
    fn invalid_responses_are_network_errors_and_not_cached() {
        for (status, body) in [
            ("503 Service Unavailable", "unavailable"),
            ("200 OK", "not json"),
            ("200 OK", r#"{"skills":[{"source":"acme/tools"}]}"#),
        ] {
            let (url, server) = serve(vec![(status, body), ("200 OK", RESULTS)]);
            let mut client = SearchClient::new(&url);
            assert_eq!(client.search("review", 50).unwrap_err().code(), "E_NETWORK");
            assert_eq!(client.search("review", 50).unwrap().len(), 1);
            server.join().unwrap();
        }
    }

    #[test]
    fn api_override_precedence() {
        assert_eq!(
            api_url(Some("http://primary"), Some("http://fallback")),
            "http://primary"
        );
        assert_eq!(api_url(None, Some("http://fallback")), "http://fallback");
        assert_eq!(
            api_url(Some(""), Some("http://fallback")),
            "http://fallback"
        );
        assert_eq!(api_url(None, None), "https://skills.sh");
    }

    #[cfg(feature = "live-tests")]
    #[test]
    fn live_search_smoke() {
        let results = SearchClient::new(DEFAULT_API).search("react", 5).unwrap();
        assert!(!results.is_empty());
        assert!(results.len() <= 5);
        assert!(
            results.iter().all(|result| {
                result.source.contains('/')
                    && !result.skill_id.is_empty()
                    && !result.name.is_empty()
            }),
            "{results:?}"
        );
    }
}
