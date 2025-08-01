#!/bin/bash

# Generate TLS certificates for otel-plugin testing
# This script creates a Certificate Authority and server certificate for testing TLS functionality

set -e

echo "🔐 Generating TLS certificates for otel-plugin testing..."

# Create certs directory if it doesn't exist
mkdir -p certs

# Clean up any existing certificates
rm -f certs/*.pem certs/*.srl server.csr

echo "📋 Generating Certificate Authority (CA)..."

# Generate CA private key
echo "  - Generating CA private key..."
openssl genrsa -out certs/ca-key.pem 2048

# Generate CA certificate
echo "  - Generating CA certificate..."
openssl req -new -x509 -key certs/ca-key.pem -out certs/ca-cert.pem -days 365 \
    -subj "/C=US/ST=Test/L=Test/O=TestCA/CN=Test-CA"

echo "🖥️  Generating Server certificate..."

# Generate server private key and certificate request
echo "  - Generating server private key and certificate request..."
openssl req -new -keyout certs/server-key.pem -out server.csr -nodes -config server.conf

# Sign server certificate with CA
echo "  - Signing server certificate with CA..."
openssl x509 -req -in server.csr -CA certs/ca-cert.pem -CAkey certs/ca-key.pem \
    -CAcreateserial -out certs/server-cert.pem -days 365 -extensions v3_req -extfile server.conf

# Clean up temporary files
rm -f server.csr

echo "✅ Certificate generation completed!"
echo ""
echo "Generated files:"
echo "  📄 certs/ca-cert.pem     - Certificate Authority certificate"
echo "  🔑 certs/ca-key.pem      - Certificate Authority private key"
echo "  📄 certs/server-cert.pem - Server certificate (signed by CA)"
echo "  🔑 certs/server-key.pem  - Server private key"
echo ""
echo "🔍 Certificate verification:"

# Verify the certificate chain
if openssl verify -CAfile certs/ca-cert.pem certs/server-cert.pem >/dev/null 2>&1; then
    echo "  ✅ Certificate chain verification: PASSED"
else
    echo "  ❌ Certificate chain verification: FAILED"
    exit 1
fi

# Display certificate details
echo ""
echo "📋 Server certificate details:"
openssl x509 -in certs/server-cert.pem -text -noout | grep -A 2 "Subject:" || true
openssl x509 -in certs/server-cert.pem -text -noout | grep -A 5 "X509v3 Subject Alternative Name:" || true

echo ""
echo "🎯 Next steps:"
echo "  1. Run './test-insecure.sh' to test HTTP connection"
echo "  2. Run './test-secure.sh' to test HTTPS/TLS connection"
echo ""
echo "⚠️  Security Note: These certificates are for TESTING ONLY!"
echo "   Do NOT use these certificates in production environments."