#include "otel_circular_buffer.h"
#include "otel_utils.h"

#if 0
// Test empty buffer
TEST(CircularBufferTest, EmptyBuffer) {
    CircularBuffer<int, 4> buffer;
    EXPECT_TRUE(buffer.empty());
    EXPECT_FALSE(buffer.full());
    EXPECT_EQ(buffer.size(), 0);
    EXPECT_EQ(buffer.capacity(), 4);
}

// Test pushing elements
TEST(CircularBufferTest, PushElements) {
    CircularBuffer<int, 4> buffer;
    buffer.push(1);
    EXPECT_FALSE(buffer.empty());
    EXPECT_EQ(buffer.size(), 1);
    
    buffer.push(2);
    buffer.push(3);
    EXPECT_EQ(buffer.size(), 3);
    EXPECT_FALSE(buffer.full());
    
    buffer.push(4);
    EXPECT_TRUE(buffer.full());
    EXPECT_EQ(buffer.size(), 4);
}

// Test popping elements
TEST(CircularBufferTest, PopElements) {
    CircularBuffer<int, 4> buffer;
    buffer.push(1);
    buffer.push(2);
    buffer.push(3);
    
    EXPECT_EQ(buffer.pop(), 1);
    EXPECT_EQ(buffer.size(), 2);
    
    EXPECT_EQ(buffer.pop(), 2);
    EXPECT_EQ(buffer.pop(), 3);
    EXPECT_TRUE(buffer.empty());
    
    EXPECT_THROW(buffer.pop(), std::out_of_range);
}

// Test wrapping behavior
TEST(CircularBufferTest, WrappingBehavior) {
    CircularBuffer<int, 4> buffer;
    buffer.push(1);
    buffer.push(2);
    buffer.push(3);
    buffer.push(4);
    buffer.pop();
    buffer.pop();
    
    buffer.push(5);
    buffer.push(6);
    
    EXPECT_EQ(buffer.pop(), 3);
    EXPECT_EQ(buffer.pop(), 4);
    EXPECT_EQ(buffer.pop(), 5);
    EXPECT_EQ(buffer.pop(), 6);
}

// Test head and tail
TEST(CircularBufferTest, HeadAndTail) {
    CircularBuffer<int, 4> buffer;
    buffer.push(1);
    EXPECT_EQ(buffer.head(), 1);
    EXPECT_EQ(buffer.tail(), 1);
    
    buffer.push(2);
    EXPECT_EQ(buffer.head(), 1);
    EXPECT_EQ(buffer.tail(), 2);
    
    buffer.pop();
    EXPECT_EQ(buffer.head(), 2);
    EXPECT_EQ(buffer.tail(), 2);
}

// Test sorting
TEST(CircularBufferTest, Sorting) {
    CircularBuffer<int, 4> buffer;
    buffer.push(3);
    buffer.push(1);
    buffer.push(4);
    buffer.push(2);
    
    buffer.sort();
    
    EXPECT_EQ(buffer.pop(), 1);
    EXPECT_EQ(buffer.pop(), 2);
    EXPECT_EQ(buffer.pop(), 3);
    EXPECT_EQ(buffer.pop(), 4);
}

// Test growing behavior
TEST(CircularBufferTest, Growing) {
    CircularBuffer<int, 4> buffer;
    for (int i = 0; i < 5; ++i) {
        buffer.push(i);
    }
    
    EXPECT_EQ(buffer.capacity(), 8);
    EXPECT_EQ(buffer.size(), 5);
    
    for (int i = 0; i < 5; ++i) {
        EXPECT_EQ(buffer.pop(), i);
    }
}

// Test iterator
TEST(CircularBufferTest, Iterator) {
    CircularBuffer<int, 4> buffer;
    buffer.push(1);
    buffer.push(2);
    buffer.push(3);
    
    auto it = buffer.begin();
    EXPECT_EQ(*it, 1);
    ++it;
    EXPECT_EQ(*it, 2);
    ++it;
    EXPECT_EQ(*it, 3);
    ++it;
    EXPECT_EQ(it, buffer.end());
}

// Test operator[]
TEST(CircularBufferTest, SubscriptOperator) {
    CircularBuffer<int, 4> buffer;
    buffer.push(1);
    buffer.push(2);
    buffer.push(3);
    
    EXPECT_EQ(buffer[0], 1);
    EXPECT_EQ(buffer[1], 2);
    EXPECT_EQ(buffer[2], 3);
    
    EXPECT_THROW(buffer[3], std::out_of_range);
}

// Test const correctness
TEST(CircularBufferTest, ConstCorrectness) {
    CircularBuffer<int, 4> buffer;
    buffer.push(1);
    buffer.push(2);
    
    const auto& const_buffer = buffer;
    
    EXPECT_EQ(const_buffer.head(), 1);
    EXPECT_EQ(const_buffer.tail(), 2);
    EXPECT_EQ(const_buffer[0], 1);
    EXPECT_EQ(const_buffer[1], 2);
    
    auto it = const_buffer.begin();
    EXPECT_EQ(*it, 1);
}

// Test with different data type (std::string)
TEST(CircularBufferTest, StringBuffer) {
    CircularBuffer<std::string, 3> buffer;
    buffer.push("hello");
    buffer.push("world");
    buffer.push("!");
    
    EXPECT_EQ(buffer.pop(), "hello");
    EXPECT_EQ(buffer.pop(), "world");
    EXPECT_EQ(buffer.pop(), "!");
}

// Test with custom initial capacity
TEST(CircularBufferTest, CustomCapacity) {
    CircularBuffer<int, 10> buffer;
    EXPECT_EQ(buffer.capacity(), 10);
    
    for (int i = 0; i < 10; ++i) {
        buffer.push(i);
    }
    
    EXPECT_TRUE(buffer.full());
    EXPECT_EQ(buffer.size(), 10);
}

// Stress test
TEST(CircularBufferTest, StressTest) {
    CircularBuffer<int, 1000> buffer;
    for (int i = 0; i < 10000; ++i) {
        buffer.push(i);
        if (i >= 1000) {
            EXPECT_EQ(buffer.pop(), i - 999);
        }
    }
    EXPECT_EQ(buffer.size(), 1000);
    EXPECT_EQ(buffer.capacity(), 1000);
}
#endif
