#ifndef ND_CIRCULAR_BUFFER_H
#define ND_CIRCULAR_BUFFER_H

#include <algorithm>

#include "absl/container/inlined_vector.h"

template <typename T, size_t N = 4> class CircularBuffer {
public:
    explicit CircularBuffer(size_t Size = N) : Buffer(Size), MaxSize(Size)
    {
    }

    void push(const T &item)
    {
        if (Full) {
            grow();
        }

        Buffer[Tail] = item;
        advanceTail();
    }

    T pop()
    {
        if (empty()) {
            throw std::out_of_range("Buffer is empty");
        }

        T item = Buffer[Head];
        advanceHead();
        return item;
    }

    const T& head() const {
        if (empty()) {
            throw std::out_of_range("Buffer is empty");
        }

        return Buffer[Head];
    }

    T& head() {
        if (empty()) {
            throw std::out_of_range("Buffer is empty");
        }

        return Buffer[Head];
    }

    const T& tail() const {
        if (empty()) {
            throw std::out_of_range("Buffer is empty");
        }

        return Full ? Buffer[Tail - 1] : Buffer[(Tail - 1 + MaxSize) % MaxSize];
    }

    T& tail() {
        if (empty()) {
            throw std::out_of_range("Buffer is empty");
        }

        return Full ? Buffer[Tail - 1] : Buffer[(Tail - 1 + MaxSize) % MaxSize];
    }

    void sort()
    {
        if (empty()) {
            return;
        }

        makeContiguous();
        std::sort(Buffer.begin() + Head, Buffer.begin() + Tail);
    }

    bool empty() const
    {
        return (!Full && (Head == Tail));
    }

    bool full() const
    {
        return Full;
    }

    size_t size() const
    {
        if (Full) {
            return MaxSize;
        }

        if (Tail >= Head) {
            return Tail - Head;
        }

        return MaxSize - (Head - Tail);
    }

    size_t capacity() const
    {
        return MaxSize;
    }

    typename std::vector<T>::const_iterator begin() const
    {
        makeContiguous();
        return Buffer.begin();
    }

    typename std::vector<T>::const_iterator end() const
    {
        makeContiguous();
        return Buffer.begin() + size();
    }

    T& operator[](size_t Index) {
        if (Index >= size()) {
            throw std::out_of_range("Index out of range");
        }

        return Buffer[(Head + Index) % MaxSize];
    }

    const T& operator[](size_t Index) const {
        if (Index >= size()) {
            throw std::out_of_range("Index out of range");
        }

        return Buffer[(Head + Index) % MaxSize];
    }

private:
    void grow() {
        makeContiguous();
        MaxSize = Buffer.size() * 2;
        Buffer.resize(MaxSize);
        Full = false;
    }

    void makeContiguous() 
    {
        if (empty() || Head == 0) {
            return;
        }

        std::rotate(Buffer.begin(), Buffer.begin() + Head, Buffer.end());

        size_t Size = size();
        Head = 0;
        Tail = Size;
    }

    void advanceTail()
    {
        if (Full) {
            Head = (Head + 1) % MaxSize;
        }
        Tail = (Tail + 1) % MaxSize;
        Full = (Head == Tail);
    }

    void advanceHead()
    {
        Head = (Head + 1) % MaxSize;
        Full = false;
    }

private:
    absl::InlinedVector<T, N> Buffer;
    size_t MaxSize;
    size_t Head = 0;
    size_t Tail = 0;
    bool Full = false;
};

#endif /* ND_CIRCULAR_BUFFER_H */
