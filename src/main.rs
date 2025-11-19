use axum::{
    extract::{Path, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, put},
    Router,
};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

type Store = Arc<RwLock<HashMap<String, String>>>;

// Latency metrics storage
#[derive(Clone)]
struct Metrics {
    latencies: Arc<RwLock<Vec<Duration>>>,
}

impl Metrics {
    fn new() -> Self {
        Self {
            latencies: Arc::new(RwLock::new(Vec::new())),
        }
    }

    fn record(&self, duration: Duration) {
        let mut latencies = self.latencies.write().unwrap();
        latencies.push(duration);
    }

    fn get_percentiles(&self) -> (f64, f64, f64, usize) {
        let mut latencies = self.latencies.write().unwrap();

        if latencies.is_empty() {
            return (0.0, 0.0, 0.0, 0);
        }

        latencies.sort();

        let len = latencies.len();
        let p50_idx = (len as f64 * 0.50) as usize;
        let p95_idx = (len as f64 * 0.95) as usize;
        let p99_idx = (len as f64 * 0.99) as usize;

        let p50 = latencies[p50_idx.min(len - 1)].as_micros() as f64 / 1000.0;
        let p95 = latencies[p95_idx.min(len - 1)].as_micros() as f64 / 1000.0;
        let p99 = latencies[p99_idx.min(len - 1)].as_micros() as f64 / 1000.0;

        (p50, p95, p99, len)
    }
}

#[tokio::main]
async fn main() {
    // Initialize tracing for logging
    tracing_subscriber::fmt::init();

    // Initialize the in-memory store
    let store: Store = Arc::new(RwLock::new(HashMap::new()));

    // Initialize metrics
    let metrics = Metrics::new();

    // Clone metrics for the background task BEFORE using it in the router
    let metrics_clone = metrics.clone();

    // Create a clone for the middleware
    let middleware_metrics = metrics.clone();

    // Build the router
    let app = Router::new()
        .route(
            "/{key}",
            put(put_handler).get(get_handler).delete(delete_handler),
        )
        .route("/metrics", get(metrics_handler))
        .layer(middleware::from_fn(move |req, next| {
            let metrics_clone = middleware_metrics.clone();
            async move {
                let start = Instant::now();
                let response = latency_middleware(req, next).await;
                let duration = start.elapsed();
                metrics_clone.record(duration);
                response
            }
        }))
        .with_state((store, metrics));

    // Spawn a background task to print metrics every 10 seconds
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        loop {
            interval.tick().await;
            let (p50, p95, p99, count) = metrics_clone.get_percentiles();
            println!("\n📊 Latency Metrics (last {} requests):", count);
            println!("   P50: {:.2}ms", p50);
            println!("   P95: {:.2}ms", p95);
            println!("   P99: {:.2}ms", p99);
        }
    });

    // Run the server
    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();

    println!("Server running on http://127.0.0.1:3000");
    println!("Metrics available at http://127.0.0.1:3000/metrics");

    axum::serve(listener, app).await.unwrap();
}

// Middleware to track latency
async fn latency_middleware(request: Request, next: Next) -> Response {
    let start = Instant::now();

    // Call the next handler
    let response = next.run(request).await;

    let duration = start.elapsed();

    // Log each request
    tracing::info!("Request took {:.2}ms", duration.as_micros() as f64 / 1000.0);

    response
}

// PUT /{key} - Create or update a key-value pair
async fn put_handler(
    State((store, _metrics)): State<(Store, Metrics)>,
    Path(key): Path<String>,
    body: String,
) -> impl IntoResponse {
    let mut map = store.write().unwrap();
    map.insert(key, body);
    StatusCode::OK
}

// GET /{key} - Retrieve a value by key
async fn get_handler(
    State((store, _metrics)): State<(Store, Metrics)>,
    Path(key): Path<String>,
) -> impl IntoResponse {
    let map = store.read().unwrap();

    match map.get(&key) {
        Some(value) => (StatusCode::OK, value.clone()),
        None => (StatusCode::NOT_FOUND, String::new()),
    }
}

// DELETE /{key} - Deletes a value by key
async fn delete_handler(
    State((store, _metrics)): State<(Store, Metrics)>,
    Path(key): Path<String>,
) -> impl IntoResponse {
    let mut map = store.write().unwrap();

    match map.remove(&key) {
        Some(_) => StatusCode::NO_CONTENT,
        None => StatusCode::NOT_FOUND,
    }
}

// GET /metrics - Get current latency metrics
async fn metrics_handler(State((_store, metrics)): State<(Store, Metrics)>) -> impl IntoResponse {
    let (p50, p95, p99, count) = metrics.get_percentiles();

    let response = format!(
        "Latency Metrics (last {} requests)\n\
         P50: {:.2}ms\n\
         P95: {:.2}ms\n\
         P99: {:.2}ms\n",
        count, p50, p95, p99
    );

    (StatusCode::OK, response)
}
