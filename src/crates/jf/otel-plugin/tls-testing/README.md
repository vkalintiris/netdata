# OTEL Plugin TLS Testing Guide

This directory contains everything needed to test the TLS functionality of the otel-plugin.

## Overview

The otel-plugin now supports secure TLS connections for production use while maintaining backward compatibility with insecure connections for development.

## Files in this Directory

- `README.md` - This file
- `generate-certificates.sh` - Script to generate test certificates
- `test-insecure.sh` - Script to test insecure HTTP connection
- `test-secure.sh` - Script to test secure HTTPS/TLS connection
- `server.conf` - OpenSSL configuration for generating proper X.509 v3 certificates
- `certs/` - Directory that will contain generated certificates (created by scripts)

## Prerequisites

1. The otel-plugin must be built with TLS support:
   ```bash
   cd /path/to/otel-plugin/workspace/root
   cargo build --bin otel-plugin
   cargo build --bin otel-test-client
   ```

2. Required tools:
   - OpenSSL (for certificate generation)
   - timeout command (usually available on Linux)

## Quick Start

1. **Generate test certificates:**
   ```bash
   ./generate-certificates.sh
   ```

2. **Test insecure connection:**
   ```bash
   ./test-insecure.sh
   ```

3. **Test secure TLS connection:**
   ```bash
   ./test-secure.sh
   ```

## Detailed Instructions

### Step 1: Generate Test Certificates

Run the certificate generation script:
```bash
./generate-certificates.sh
```

This will create:
- `certs/ca-cert.pem` - Certificate Authority certificate
- `certs/ca-key.pem` - Certificate Authority private key
- `certs/server-cert.pem` - Server certificate (signed by CA)
- `certs/server-key.pem` - Server private key

### Step 2: Test Insecure Connection

```bash
./test-insecure.sh
```

**Expected output:**
```
Starting otel-plugin in insecure mode...
TLS disabled, using insecure connection on endpoint: 0.0.0.0:21213
TRUST_DURATIONS 1
Testing insecure connection...
Sending test metric to insecure endpoint...
✅ Successfully sent metric! Response: ExportMetricsServiceResponse { partial_success: None }
Test completed successfully!
```

### Step 3: Test Secure TLS Connection

```bash
./test-secure.sh
```

**Expected output:**
```
Starting otel-plugin in secure mode...
Loading TLS certificate from: ./tls-testing/certs/server-cert.pem
Loading TLS private key from: ./tls-testing/certs/server-key.pem
TLS enabled on endpoint: 0.0.0.0:21214
TRUST_DURATIONS 1
Testing secure TLS connection...
Sending test metric to secure endpoint...
FlattenedPoint {
    attributes: {
        "resource.attributes.service.name": String("test-client"),
    },
    nd_instance_name: "test_metric.65f9f7bfd45ee28a",
    nd_dimension_name: "value",
    metric_name: "test_metric",
    metric_description: "A test metric",
    metric_unit: "1",
    metric_type: "gauge",
    metric_time_unix_nano: 1754041712958906102,
    metric_value: 42.0,
    metric_is_monotonic: None,
}
✅ Successfully sent metric! Response: ExportMetricsServiceResponse { partial_success: None }
Test completed successfully!
```

## Manual Testing Commands

If you prefer to run commands manually:

### Build the binaries:
```bash
cd /path/to/workspace/root
cargo build --bin otel-plugin
cargo build --bin otel-test-client
```

### Generate certificates manually:
```bash
cd tls-testing

# Generate CA private key
openssl genrsa -out certs/ca-key.pem 2048

# Generate CA certificate
openssl req -new -x509 -key certs/ca-key.pem -out certs/ca-cert.pem -days 365 \
    -subj "/C=US/ST=Test/L=Test/O=TestCA/CN=Test-CA"

# Generate server private key
openssl req -new -keyout certs/server-key.pem -out server.csr -nodes -config server.conf

# Sign server certificate with CA
openssl x509 -req -in server.csr -CA certs/ca-cert.pem -CAkey certs/ca-key.pem \
    -CAcreateserial -out certs/server-cert.pem -days 365 -extensions v3_req -extfile server.conf

# Clean up
rm server.csr
```

### Test insecure connection manually:
```bash
# Terminal 1: Start server
./target/debug/otel-plugin --otel-metrics-print-flattened

# Terminal 2: Send test metric
./target/debug/otel-test-client insecure
```

### Test secure connection manually:
```bash
# Terminal 1: Start server with TLS
./target/debug/otel-plugin \
    --otel-endpoint "0.0.0.0:21214" \
    --otel-tls-enabled \
    --otel-tls-cert-path ./tls-testing/certs/server-cert.pem \
    --otel-tls-key-path ./tls-testing/certs/server-key.pem \
    --otel-metrics-print-flattened

# Terminal 2: Send test metric over TLS
ENDPOINT=https://localhost:21214 ./target/debug/otel-test-client secure ./tls-testing/certs/ca-cert.pem
```

## Configuration Options

The otel-plugin supports the following TLS-related options:

- `--otel-tls-enabled` - Enable TLS/SSL for secure connections
- `--otel-tls-cert-path <PATH>` - Path to TLS certificate file (required when TLS enabled)
- `--otel-tls-key-path <PATH>` - Path to TLS private key file (required when TLS enabled)
- `--otel-tls-ca-cert-path <PATH>` - Path to TLS CA certificate file for client authentication (optional)

## Production Usage

For production use:

1. **Generate proper certificates** from a trusted Certificate Authority or use tools like Let's Encrypt
2. **Secure the private key** with appropriate file permissions (600)
3. **Use strong cipher suites** (handled automatically by rustls)
4. **Consider client certificate authentication** for additional security using `--otel-tls-ca-cert-path`

## Troubleshooting

### Common Issues:

1. **"Address already in use" error**: Change the port using `--otel-endpoint "0.0.0.0:DIFFERENT_PORT"`

2. **Certificate validation errors**: Ensure certificates are properly formatted X.509 v3 certificates

3. **Permission denied**: Make sure certificate files are readable by the otel-plugin process

4. **Connection refused**: Verify the server is listening on the correct endpoint and firewall allows the connection

### Debug Commands:

```bash
# Verify certificate format
openssl x509 -in certs/server-cert.pem -text -noout

# Check certificate chain
openssl verify -CAfile certs/ca-cert.pem certs/server-cert.pem

# Test server connectivity
openssl s_client -connect localhost:21214 -CAfile certs/ca-cert.pem
```

## Security Notes

- Test certificates are self-signed and should **NOT** be used in production
- Private keys are generated without passphrases for testing convenience
- In production, secure your private keys with proper permissions and consider using passphrases
- The CA certificate allows client authentication - protect it appropriately