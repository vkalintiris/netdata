#!/bin/bash

# Validate otel-plugin TLS testing setup
# This script checks if all prerequisites are met for testing

echo "🔍 Validating otel-plugin TLS testing setup..."
echo ""

ERRORS=0

# Check if we're in the right directory
if [ ! -f "README.md" ] || [ ! -f "server.conf" ]; then
    echo "❌ Error: Not in the correct directory"
    echo "   Please run this script from the tls-testing directory"
    ERRORS=$((ERRORS + 1))
fi

# Check for required binaries
echo "📦 Checking binaries..."
if [ -f "../../target/debug/otel-plugin" ]; then
    echo "   ✅ otel-plugin binary found"
else
    echo "   ❌ otel-plugin binary not found at ../../target/debug/otel-plugin"
    echo "      Run: cargo build --bin otel-plugin"
    ERRORS=$((ERRORS + 1))
fi

if [ -f "../../target/debug/otel-test-client" ]; then
    echo "   ✅ otel-test-client binary found"
else
    echo "   ❌ otel-test-client binary not found at ../../target/debug/otel-test-client"
    echo "      Run: cargo build --bin otel-test-client"
    ERRORS=$((ERRORS + 1))
fi

# Check for OpenSSL
echo ""
echo "🔧 Checking tools..."
if command -v openssl >/dev/null 2>&1; then
    echo "   ✅ OpenSSL found: $(openssl version)"
else
    echo "   ❌ OpenSSL not found"
    echo "      Install OpenSSL to generate certificates"
    ERRORS=$((ERRORS + 1))
fi

if command -v timeout >/dev/null 2>&1; then
    echo "   ✅ timeout command found"
else
    echo "   ❌ timeout command not found"
    echo "      This is usually available on Linux systems"
    ERRORS=$((ERRORS + 1))
fi

# Check certificates
echo ""
echo "📄 Checking certificates..."
if [ -f "certs/ca-cert.pem" ] && [ -f "certs/server-cert.pem" ] && [ -f "certs/server-key.pem" ]; then
    echo "   ✅ Test certificates found"
    
    # Verify certificate chain if OpenSSL is available
    if command -v openssl >/dev/null 2>&1; then
        if openssl verify -CAfile certs/ca-cert.pem certs/server-cert.pem >/dev/null 2>&1; then
            echo "   ✅ Certificate chain validation: PASSED"
        else
            echo "   ⚠️  Certificate chain validation: FAILED"
            echo "      Certificates may need to be regenerated"
        fi
    fi
else
    echo "   ⚠️  Test certificates not found"
    echo "      Run: ./generate-certificates.sh"
fi

# Check scripts
echo ""
echo "📜 Checking test scripts..."
for script in generate-certificates.sh test-insecure.sh test-secure.sh run-all-tests.sh; do
    if [ -f "$script" ] && [ -x "$script" ]; then
        echo "   ✅ $script is executable"
    elif [ -f "$script" ]; then
        echo "   ⚠️  $script found but not executable"
        echo "      Run: chmod +x $script"
    else
        echo "   ❌ $script not found"
        ERRORS=$((ERRORS + 1))
    fi
done

echo ""
echo "=================================================="
if [ $ERRORS -eq 0 ]; then
    echo "✅ Setup validation: PASSED"
    echo ""
    echo "🎯 Ready to run tests! Try:"
    echo "   ./run-all-tests.sh     - Run all tests"
    echo "   ./test-insecure.sh     - Test HTTP only"
    echo "   ./test-secure.sh       - Test HTTPS only"
else
    echo "❌ Setup validation: FAILED ($ERRORS errors)"
    echo ""
    echo "🔧 Please fix the errors above before running tests."
fi
echo "=================================================="