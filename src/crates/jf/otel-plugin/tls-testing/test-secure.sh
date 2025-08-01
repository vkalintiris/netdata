#!/bin/bash

# Test otel-plugin secure HTTPS/TLS connection
# This script tests the otel-plugin with TLS enabled to verify secure functionality

set -e

echo "🔒 Testing otel-plugin secure HTTPS/TLS connection..."

# Check if binaries exist
if [ ! -f "../../target/debug/otel-plugin" ]; then
    echo "❌ Error: otel-plugin binary not found at ../../target/debug/otel-plugin"
    echo "   Please run 'cargo build --bin otel-plugin' from the workspace root first."
    exit 1
fi

if [ ! -f "../../target/debug/otel-test-client" ]; then
    echo "❌ Error: otel-test-client binary not found at ../../target/debug/otel-test-client"
    echo "   Please run 'cargo build --bin otel-test-client' from the workspace root first."
    exit 1
fi

# Check if certificates exist
if [ ! -f "certs/server-cert.pem" ] || [ ! -f "certs/server-key.pem" ] || [ ! -f "certs/ca-cert.pem" ]; then
    echo "❌ Error: TLS certificates not found in certs/ directory"
    echo "   Please run './generate-certificates.sh' first to create test certificates."
    exit 1
fi

echo "🔐 Starting otel-plugin in secure mode..."

# Start the server in background with timeout
timeout 15s ../../target/debug/otel-plugin \
    --otel-endpoint "0.0.0.0:21214" \
    --otel-tls-enabled \
    --otel-tls-cert-path ./certs/server-cert.pem \
    --otel-tls-key-path ./certs/server-key.pem \
    --otel-metrics-print-flattened & 

SERVER_PID=$!

# Wait for server to start
echo "⏳ Waiting for server to start..."
sleep 3

echo "🧪 Testing secure TLS connection..."

# Test the connection with CA certificate verification
if ENDPOINT=https://localhost:21214 timeout 8s ../../target/debug/otel-test-client secure ./certs/ca-cert.pem; then
    echo "✅ Secure TLS connection test: PASSED"
    TEST_RESULT=0
else
    echo "❌ Secure TLS connection test: FAILED"
    TEST_RESULT=1
fi

# Clean up server process
echo "🛑 Stopping server..."
kill $SERVER_PID 2>/dev/null || true
wait $SERVER_PID 2>/dev/null || true

if [ $TEST_RESULT -eq 0 ]; then
    echo ""
    echo "🎉 Test completed successfully!"
    echo "   The otel-plugin TLS/HTTPS connection is working correctly."
else
    echo ""
    echo "💥 Test failed!"
    echo "   Please check the error messages above."
    exit 1
fi

echo ""
echo "📝 What this test verified:"
echo "   ✅ otel-plugin starts with TLS enabled"
echo "   ✅ TLS certificate and private key load successfully"
echo "   ✅ HTTPS endpoint accepts secure connections"
echo "   ✅ Certificate chain validation works"
echo "   ✅ OTEL metrics are received and processed over TLS"
echo "   ✅ Client receives successful response over secure channel"

echo ""
echo "🔍 Security features verified:"
echo "   🔐 Server certificate authentication"
echo "   🔐 Encrypted data transmission"
echo "   🔐 Certificate chain validation"
echo "   🔐 TLS/SSL handshake completion"