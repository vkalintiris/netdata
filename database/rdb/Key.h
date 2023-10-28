#ifndef RDB_KEY_H
#define RDB_KEY_H

#include <array>
#include "rdb-common.h"
#include "uuid.h"

namespace rdb {

class UuidKey
{
public:
    constexpr static size_t Bytes = sizeof(uuid_t);

public:
    [[nodiscard]] inline static const UuidKey min()
    {
        std::array<char, Bytes> v;
        std::fill(v.begin(), v.end(), 0);
        return UuidKey(v);
    }

    [[nodiscard]] inline static const UuidKey max()
    {
        std::array<char, Bytes> v;
        std::fill(v.begin(), v.end(), 0xFF);
        return UuidKey(v);
    }

public:
    inline UuidKey(const std::array<char, Bytes> &AR)
    {
        memcpy(&Scratch[0], AR.data(), Bytes);
    }

    inline UuidKey(uuid_t *uuid)
    {
        memcpy(&Scratch[0], uuid, Bytes);
    }

    inline UuidKey(const Slice &S)
    {
        memcpy(&Scratch[0], S.data(), Bytes);
    }

    [[nodiscard]] inline const Slice slice() const
    {
        return Slice(reinterpret_cast<const char *>(Scratch.data()), Bytes);
    }

    inline void uuid(uuid_t *uuid) const
    {
        memcpy(uuid, Scratch.data(), Bytes);
    }

private:
    std::array<unsigned char, UuidKey::Bytes> Scratch;
};

/**
 * @brief Represents a metric key in the RocksDB database.
 *
 * A key is composed of 2 fields: `group-id` and `metric-id`
 * The type of each field is `uint32_t` and is saved in big-endian order.
 *
*/
class MetricKey
{
public:
    /**
     * @brief Number of fields in the key.
    */
    constexpr static size_t Fields = 2;

    /**
     * @brief Total size of the key in bytes.
    */
    constexpr static size_t Bytes = Fields * sizeof(uint32_t);

private:
    constexpr static size_t GroupIdField = 0;
    constexpr static size_t MetricIdField = 1;

private:
    [[nodiscard]] inline uint32_t field(size_t i) const
    {
        assert(i < 2);

        uint32_t f;
        memcpy(&f, &Scratch[i * sizeof(uint32_t)], sizeof(uint32_t));
        return be32toh(f);
    }

public:
    /**
     * @brief Min key with GroupID, MetricID and PointInTime equal to 0.
    */
    static const MetricKey min()
    {
        return MetricKey(0, 0);
    }

    /**
     * @brief Max key with GroupID, MetricID and PointInTime equal to ~0u.
    */
    static const MetricKey max()
    {
        uint32_t m = std::numeric_limits<uint32_t>::max();
        return MetricKey(m, m);
    }

    inline MetricKey() = default;

    /**
     * @brief Constructs a Key with the given field values.
     * @param gid GroupId value.
     * @param mid MetricId value.
     * @param pit PointInTime value.
    */
    inline MetricKey(uint32_t gid, uint32_t mid)
    {
        gid = htobe32(gid);
        mid = htobe32(mid);

        memcpy(&Scratch[GroupIdField * sizeof(uint32_t)], &gid, sizeof(uint32_t));
        memcpy(&Scratch[MetricIdField * sizeof(uint32_t)], &mid, sizeof(uint32_t));
    }

    /**
     * @brief Constructor to initialize the key from a Slice.
     * @param S The Slice containing the key bytes.
    */
    inline MetricKey(const Slice &S)
    {
        memcpy(&Scratch[0], S.data(), MetricKey::Bytes);
    }

    /**
     * @brief Constructor to initialize the key from a char array.
     * @param S The array containing the key bytes.
    */
    inline MetricKey(const std::array<char, MetricKey::Bytes> &AR)
    {
        assert(AR.size() >= MetricKey::Bytes);
        memcpy(&Scratch[0], AR.data(), AR.size());
    }

    /**
     * @brief Returns a Slice representation of the key.
     * @return The Slice representation of the key.
    */
    [[nodiscard]] inline const Slice slice() const
    {
        return Slice(Scratch.data(), Scratch.size());
    }

    /**
     * @brief Gets the GroupId component of the key.
     * @return The GroupId value.
    */
    [[nodiscard]] inline uint32_t gid() const
    {
        return field(GroupIdField);
    }

    /**
     * @brief Gets the MetricId component of the key.
     * @return The MetricId value.
    */
    [[nodiscard]] inline uint32_t mid() const
    {
        return field(MetricIdField);
    }

    /**
     * @brief Returns a string representation of the key.
     * @param hex If true, display values in hexadecimal.
     * @return The string representation of the key.
    */
    [[nodiscard]] std::string toString(bool hex = false) const
    {
        std::array<char, 1024> buf;

        if (hex)
        {
            snprintfz(buf.data(), buf.size() - 1, "gid=%u, mid=%u (0x%s)",
                      gid(), mid(), slice().ToString(true).c_str());
        }
        else
        {
            snprintfz(buf.data(), buf.size() - 1, "gid=%u, mid=%u",
                      gid(), mid());
        }

        return std::string(buf.data());
    }

private:
    /**
     * @brief Internal storage for the key data.
    */
    std::array<char, MetricKey::Bytes> Scratch;
};

/**
 * @brief Represents a key in the RocksDB database.
 *
 * A key is composed of three fields: `group-id`, `metric-id` and `point-in-time`.
 * The type of each field is `uint32_t` and is saved in big-endian order.
 *
*/
class Key
{
public:
    /**
     * @brief Number of fields in the key.
    */
    constexpr static size_t Fields = 3;

    /**
     * @brief Total size of the key in bytes.
    */
    constexpr static size_t Bytes = Fields * sizeof(uint32_t);

private:
    constexpr static size_t GroupIdField = 0;
    constexpr static size_t MetricIdField = 1;
    constexpr static size_t PointInTimeField = 2;

private:
    [[nodiscard]] inline uint32_t field(size_t i) const
    {
        assert(i < 3);

        uint32_t f;
        memcpy(&f, &Scratch[i * sizeof(uint32_t)], sizeof(uint32_t));
        return be32toh(f);
    }

public:
    /**
     * @brief Min Key with GroupID, MetricID and PointInTime equal to 0.
    */
    static const Key min()
    {
        return Key(0, 0, 0);
    }

    /**
     * @brief Max Key with GroupID, MetricID and PointInTime equal to ~0u.
    */
    static const Key max()
    {
        uint32_t m = std::numeric_limits<uint32_t>::max();
        return Key(m, m, m);
    }

    inline Key() = default;

    /**
     * @brief Constructs a Key with the given field values.
     * @param gid GroupId value.
     * @param mid MetricId value.
     * @param pit PointInTime value.
    */
    inline Key(uint32_t gid, uint32_t mid, uint32_t pit)
    {
        gid = htobe32(gid);
        mid = htobe32(mid);
        pit = htobe32(pit);

        memcpy(&Scratch[GroupIdField * sizeof(uint32_t)], &gid, sizeof(uint32_t));
        memcpy(&Scratch[MetricIdField * sizeof(uint32_t)], &mid, sizeof(uint32_t));
        memcpy(&Scratch[PointInTimeField * sizeof(uint32_t)], &pit, sizeof(uint32_t));
    }

    /**
     * @brief Constructor to initialize the key from a Slice.
     * @param S The Slice containing the key bytes.
    */
    inline Key(const Slice &S)
    {
        memcpy(&Scratch[0], S.data(), rdb::Key::Bytes);
    }

    /**
     * @brief Constructor to initialize the key from a char array.
     * @param S The array containing the key bytes.
    */
    inline Key(const std::array<char, Key::Bytes> &AR)
    {
        memcpy(&Scratch[0], AR.data(), AR.size());
    }

    /**
     * @brief Returns a Slice representation of the key.
     * @return The Slice representation of the key.
    */
    [[nodiscard]] inline const Slice slice() const
    {
        return Slice(Scratch.data(), Scratch.size());
    }

    /**
     * @brief Gets the GroupId component of the key.
     * @return The GroupId value.
    */
    [[nodiscard]] inline uint32_t gid() const
    {
        return field(GroupIdField);
    }


    /**
     * @brief Gets the MetricId component of the key.
     * @return The MetricId value.
    */
    [[nodiscard]] inline uint32_t mid() const
    {
        return field(MetricIdField);
    }

    /**
     * @brief Gets the PointInTime component of the key.
     * @return The PointInTime value.
    */
    [[nodiscard]] inline uint32_t pit() const
    {
        return field(PointInTimeField);
    }

    /**
     * @brief Returns a string representation of the key.
     * @param hex If true, display values in hexadecimal.
     * @return The string representation of the key.
    */
    [[nodiscard]] std::string toString(bool hex = false) const
    {
        std::array<char, 1024> buf;

        if (hex)
        {
            snprintfz(buf.data(), buf.size() - 1, "gid=%u, mid=%u, pit=%u (0x%s)",
                      gid(), mid(), pit(), slice().ToString(true).c_str());
        }
        else
        {
            snprintfz(buf.data(), buf.size() - 1, "gid=%u, mid=%u, pit=%u",
                      gid(), mid(), pit());
        }

        return std::string(buf.data());
    }

private:
    /**
     * @brief Internal storage for the key data.
    */
    std::array<char, Key::Bytes> Scratch;
};

} // namespace rdb

#endif /* RDB_KEY_H */
