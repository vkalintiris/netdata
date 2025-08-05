# Hello Service - Enhanced Netdata Plugin

A demonstration service showcasing the enhanced Netdata plugin runtime with dual-context architecture, function registry, timeout handling, and cancellation support.

## 🚀 Quick Start

Build and run the service:
```bash
cargo build --release -p hello-service
./target/release/hello-service
```

The service will start and display usage information with example function calls.

## 📋 Available Functions

| Function | Timeout | Description |
|----------|---------|-------------|
| `hello` | 10s | Returns a greeting with plugin statistics |
| `stats` | 5s | Shows detailed plugin and function statistics |
| `processes` | 30s | Lists demo processes with concurrent function info |
| `slow` | 5s | Demonstrates timeout handling (intentionally slow) |

## 💡 Example Usage

The service uses the Netdata plugin protocol. Paste these commands into stdin:

```
FUNCTION test_001 hello 10 "test_user"
FUNCTION test_002 stats 5 ""
FUNCTION test_003 processes 30 ""
FUNCTION test_004 slow 5 ""
```

## 📝 Protocol Format

**Input:**
```
FUNCTION <transaction_id> <function_name> <timeout_seconds> <source>
```

**Output:**
```
FUNCTION_RESULT <transaction_id> <status_code> <content_type> <expires> <payload>
```

## 🧪 Testing Scenarios

### Basic Function Call
```bash
echo 'FUNCTION tx001 hello 10 "user"' | ./target/release/hello-service
```

### Timeout Testing
```bash
echo 'FUNCTION tx002 slow 5 ""' | ./target/release/hello-service
```
*Should timeout after 5 seconds with status 408*

### Statistics Monitoring
```bash
# Run multiple calls then check stats
echo 'FUNCTION tx003 hello 10 ""' | ./target/release/hello-service
echo 'FUNCTION tx004 stats 5 ""' | ./target/release/hello-service
```

### Concurrent Function Testing
```bash
# In separate terminals, or script with background processes
echo 'FUNCTION tx005 processes 30 ""' &
echo 'FUNCTION tx006 hello 10 ""' &
echo 'FUNCTION tx007 stats 5 ""' &
```

## 🏗️ Architecture Features Demonstrated

- **Dual Context Architecture**: Plugin context + Function context
- **Function Registry**: Per-function handlers with metadata
- **Transaction Tracking**: Full lifecycle management
- **Timeout Handling**: Runtime-level and function-level timeout detection
- **Cancellation Support**: Infrastructure for function cancellation
- **Statistics Tracking**: Comprehensive plugin and function metrics
- **Enhanced Error Handling**: Proper error propagation and logging

## 🔍 Response Examples

**Hello Function:**
```
FUNCTION_RESULT tx001 200 text/plain 0 Hello from hello-service SDK! You called function 'hello' from source: 'user'.
This is implemented using the new SDK with enhanced runtime.
Transaction ID: tx001
Plugin: hello-service-enhanced | Total calls: 1 | Active transactions: 0
Function elapsed: 2ms
```

**Stats Function (JSON):**
```json
{
  "plugin": "hello-service-enhanced",
  "version": "0.1.0",
  "function": "stats",
  "transaction": "tx002",
  "function_metadata": {
    "start_time": 1692000000,
    "elapsed_ms": 5,
    "timeout": 5,
    "is_timed_out": false
  },
  "statistics": {
    "total_calls": 2,
    "successful_calls": 2,
    "failed_calls": 0,
    "timed_out_calls": 0,
    "active_transactions": 0
  }
}
```

**Timeout Response:**
```
FUNCTION_RESULT tx003 408 text/plain 0 Function 'slow' timed out after 5 seconds
```