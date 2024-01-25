use prometheus::{Encoder, Gauge, TextEncoder};
use std::error::Error;
use std::net::SocketAddr;
use tokio::io::AsyncReadExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::time::{interval, Duration};
use warp::Filter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Create a Prometheus Gauge for pressure_0 and pressure_1
    let pressure_0 = Gauge::new("pressure_0", "Pressure 0 gauge").unwrap();
    let pressure_1 = Gauge::new("pressure_1", "Pressure 1 gauge").unwrap();

    let dummy_p_0: f64 = 1.22e-10;
    let dummy_p_1: f64 = 1.45e-4;

    pressure_0.set(dummy_p_0);
    pressure_1.set(dummy_p_1);

    // Clone the gauges for updating from the TCP server
    let pressure_0_clone = pressure_0.clone();
    let pressure_1_clone = pressure_1.clone();

    // Start the TCP server
    tokio::spawn(tcp_server(pressure_0_clone, pressure_1_clone));

    // Start the Prometheus server
    let prometheus_server_result = prometheus_server(pressure_0, pressure_1).await;

    match prometheus_server_result {
        Ok(_) => {}
        Err(err) => {
            eprintln!("Error starting Prometheus server: {}", err);
        }
    }

    Ok(())
}

async fn tcp_server(pressure_0: Gauge, pressure_1: Gauge) {
    // Bind the TCP server to the address 127.0.0.1:8080, IP of local host
    let listener = TcpListener::bind("127.0.0.1:8080").await.unwrap();

    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(handle_connection(
            stream,
            pressure_0.clone(),
            pressure_1.clone(),
        ));
    }
}

async fn handle_connection(
    stream: TcpStream,
    pressure_0: Gauge,
    pressure_1: Gauge,
) -> Result<(), Box<dyn Error + Send + Sync + 'static>> {
    // Use a channel to send data from the TCP server to the Prometheus server
    let (tx, mut rx) = mpsc::channel(1);

    // Spawn a task to read data from the TCP stream
    tokio::spawn(read_tcp_data(tx.clone(), stream));

    // Periodically update Prometheus gauges with the received data
    let mut interval = interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = interval.tick() => {
                // Receive data from the channel
                if let Some(data) = rx.recv().await {
                    // Update gauges with received data
                    pressure_0.set(data.pressure_0);
                    pressure_1.set(data.pressure_1);
                }
            }
        }
    }
}

async fn read_tcp_data(tx: mpsc::Sender<TcpData>, mut stream: TcpStream) {
    // Buffer to hold incoming data
    let mut buffer = [0u8; 1024];

    loop {
        match stream.read(&mut buffer).await {
            Ok(0) => {
                // Connection closed by client
                println!("Connection closed by client");
                break;
            }
            Ok(n) => {
                // Convert bytes to string
                let data = String::from_utf8_lossy(&buffer[..n]);

                // Parse the received data and update the channel
                println!("Updating channel...");
                update_channel(&tx, (&data).to_string());
            }
            Err(e) => {
                eprintln!("Error reading from TCP stream: {}", e);
                break;
            }
        }
    }
}

fn update_channel(tx: &mpsc::Sender<TcpData>, data: String) {
    // Parse the received data and update the channel
    let parts: Vec<&str> = data.trim().split(';').collect();

    if parts.len() == 5 {
        if let (Ok(p0), Ok(p1)) = (parts[1].parse::<f64>(), parts[3].parse::<f64>()) {
            let _ = tx.send(TcpData {
                pressure_0: p0,
                pressure_1: p1,
            });
        } else {
            eprintln!("Error parsing numbers from data: {}", data);
        }
    } else {
        eprintln!("Invalid data format: {}", data);
    }
}

async fn prometheus_server(
    pressure_0: Gauge,
    pressure_1: Gauge,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    // Register gauges with Prometheus
    prometheus::register(Box::new(pressure_0.clone()))?;
    prometheus::register(Box::new(pressure_1.clone()))?;

    // Start the Prometheus server on local host on port 9090 (Unoficially registered port for Prometheus servers)
    let addr: SocketAddr = "127.0.0.1:9090".parse()?;
    warp::serve(prometheus_metrics_handler()).run(addr).await;

    Ok(())
}

fn prometheus_metrics_handler(
) -> impl warp::Filter<Extract = impl warp::Reply, Error = warp::Rejection> + Clone {
    // Define a Warp filter for the /metrics endpoint
    warp::path!("metrics")
        .map(move || {
            // Handler function to generate Prometheus metrics response
            let mut buffer = Vec::new();
            let encoder = TextEncoder::new();

            // Gather metrics and encode them into the response buffer
            let metric_families = prometheus::gather();
            encoder.encode(&metric_families, &mut buffer).unwrap();

            // Convert the response buffer to a String
            String::from_utf8(buffer).unwrap()
        })
        .boxed()
}

// Struct to hold TCP data
#[derive(Debug, Clone, Copy)]
struct TcpData {
    pressure_0: f64,
    pressure_1: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::sleep;

    // Helper function to make an HTTP request to the /metrics endpoint
    async fn get_metrics(endpoint: &str) -> String {
        let client = reqwest::Client::new();
        let response = client.get(endpoint).send().await.unwrap();
        assert!(response.status().is_success());
        response.text().await.unwrap()
    }

    #[tokio::test]
    async fn test_prometheus_server() {
        let pressure_0 = Gauge::new("pressure_0", "Pressure 0 gauge").unwrap();
        let pressure_1 = Gauge::new("pressure_1", "Pressure 1 gauge").unwrap();

        let prometheus_server_handle =
            tokio::spawn(prometheus_server(pressure_0.clone(), pressure_1.clone()));

        // Ensure the Prometheus server is running by making a request to the /metrics endpoint
        let response = reqwest::get("http://127.0.0.1:9090/metrics").await.unwrap();
        assert!(response.status().is_success());

        // Clean up the Prometheus server
        prometheus_server_handle.abort();
    }

    #[tokio::test]
    async fn test_tcp_server_and_prometheus_integration() {
        let pressure_0 = Gauge::new("pressure_0", "Pressure 0 gauge").unwrap();
        let pressure_1 = Gauge::new("pressure_1", "Pressure 1 gauge").unwrap();

        let dummy_p_0: f64 = 1.22e-10;
        let dummy_p_1: f64 = 1.45e-4;

        pressure_0.set(dummy_p_0);
        pressure_1.set(dummy_p_1);

        // Clone the gauges for updating from the TCP server
        let pressure_0_clone = pressure_0.clone();
        let pressure_1_clone = pressure_1.clone();

        let tcp_server_handle = tokio::spawn(tcp_server(pressure_0_clone, pressure_1_clone));
        let prometheus_server_handle =
            tokio::spawn(prometheus_server(pressure_0.clone(), pressure_1.clone()));

        // Ensure that Prometheus metrics are updated after receiving data from the TCP server
        sleep(Duration::from_secs(3)).await; // Wait for the metrics to be updated

        // Try to access the values of the metrics
        let metrics = get_metrics("http://127.0.0.1:9090/metrics").await;

        // Print statements for when the assertion below fails
        println!("{}", metrics);
        println!("pressure_0: {}", pressure_0.get());
        println!("pressure_1: {}", pressure_1.get());

        // Test the metrics for the wanted pressure values
        assert!(metrics.contains("pressure_0 0.000000000122"));
        assert!(metrics.contains("pressure_1 0.000145"));

        // Clean up the servers
        tcp_server_handle.abort();
        prometheus_server_handle.abort();
    }
}
