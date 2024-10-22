#ifndef ND_CIRCULAR_BUFFER_H
#define ND_CIRCULAR_BUFFER_H

#include "absl/container/inlined_vector.h"

template <typename T, size_t N = 4> class SortedContainer {
    static_assert(std::is_copy_constructible_v<T>, "Type T must be copy constructible");

public:
    SortedContainer() = default;

    void push(const T &Item)
    {
        auto It = std::lower_bound(IV.begin(), IV.end(), Item);
        IV.insert(It, Item);
    }

    // Emplace construct a new item in-place
    template <typename... Args> void emplace(Args &&...args)
    {
        T Item(std::forward<Args>(args)...);
        push(Item);
    }

    T pop()
    {
        assert(!empty() && "Container is empty");

        T item = IV.front();
        IV.erase(IV.begin());
        return item;
    }

    const T& peek() const
    {
        assert(!empty() && "Container is empty");

        return IV.front();
    }

    size_t size() const noexcept
    {
        return IV.size();
    }

    bool empty() const noexcept
    {
        return IV.empty();
    }

    void clear() noexcept
    {
        IV.clear();
    }

    const T &operator[](size_t index) const
    {
        return IV[index];
    }

    const T &at(size_t index) const
    {
        return IV.at(index);
    }

    auto begin() const noexcept
    {
        return IV.begin();
    }

    auto end() const noexcept
    {
        return IV.end();
    }

    auto cbegin() const noexcept
    {
        return IV.cbegin();
    }

    auto cend() const noexcept
    {
        return IV.cend();
    }

    size_t capacity() const noexcept
    {
        return IV.capacity();
    }

    void reserve(size_t n)
    {
        IV.reserve(n);
    }

    void shrink_to_fit()
    {
        IV.shrink_to_fit();
    }

private:
    absl::InlinedVector<T, N> IV;
};

#endif /* ND_CIRCULAR_BUFFER_H */
