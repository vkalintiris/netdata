#!/bin/bash

# Run all otel-plugin TLS tests
# This script runs both insecure and secure tests in sequence

set -e

echo "🚀 Running complete otel-plugin TLS test suite..."
echo ""

# Test 1: Generate certificates
echo "=================================================="
echo "📋 Step 1: Generating test certificates"
echo "=================================================="
./generate-certificates.sh

echo ""
echo "=================================================="
echo "🌐 Step 2: Testing insecure HTTP connection"
echo "=================================================="
./test-insecure.sh

echo ""
echo "=================================================="
echo "🔒 Step 3: Testing secure HTTPS/TLS connection"
echo "=================================================="
./test-secure.sh

echo ""
echo "=================================================="
echo "🎉 ALL TESTS COMPLETED SUCCESSFULLY!"
echo "=================================================="
echo ""
echo "✅ Certificate generation: PASSED"
echo "✅ Insecure HTTP connection: PASSED"
echo "✅ Secure HTTPS/TLS connection: PASSED"
echo ""
echo "🔧 The otel-plugin TLS implementation is working correctly!"
echo ""
echo "📁 Test artifacts saved in:"
echo "   - ./certs/ - TLS certificates"
echo "   - ./README.md - Detailed documentation"