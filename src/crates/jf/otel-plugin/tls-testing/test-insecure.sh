#!/bin/bash

# Test otel-plugin insecure HTTP connection
# This script tests the otel-plugin without TLS to verify basic functionality

set -e

echo "🌐 Testing otel-plugin insecure HTTP connection..."

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

echo "📡 Starting otel-plugin in insecure mode..."

# Start the server in background with timeout
timeout 15s ../../target/debug/otel-plugin \
    --otel-endpoint "0.0.0.0:21213" \
    --otel-metrics-print-flattened & 

SERVER_PID=$!

# Wait for server to start
echo "⏳ Waiting for server to start..."
sleep 3

echo "🧪 Testing insecure connection..."

# Test the connection
if timeout 8s ../../target/debug/otel-test-client insecure; then
    echo "✅ Insecure connection test: PASSED"
    TEST_RESULT=0
else
    echo "❌ Insecure connection test: FAILED"
    TEST_RESULT=1
fi

# Clean up server process
echo "🛑 Stopping server..."
kill $SERVER_PID 2>/dev/null || true
wait $SERVER_PID 2>/dev/null || true

if [ $TEST_RESULT -eq 0 ]; then
    echo ""
    echo "🎉 Test completed successfully!"
    echo "   The otel-plugin insecure HTTP connection is working correctly."
else
    echo ""
    echo "💥 Test failed!"
    echo "   Please check the error messages above."
    exit 1
fi

echo ""
echo "📝 What this test verified:"
echo "   ✅ otel-plugin starts without TLS"
echo "   ✅ HTTP endpoint accepts connections"
echo "   ✅ OTEL metrics are received and processed"
echo "   ✅ Client receives successful response"