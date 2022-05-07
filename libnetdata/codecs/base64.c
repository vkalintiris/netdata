
// SPDX-License-Identifier: GPL-3.0-or-later

#include "../libnetdata.h"

// Base-64 decoder.
// Note: This is non-validating, invalid input will be decoded without an error.
//       Challenges are packed into json strings so we don't skip newlines.
//       Size errors (i.e. invalid input size or insufficient output space) are caught.
size_t base64_decode(unsigned char *input, size_t input_size,
                     unsigned char *output, size_t output_size)
{
    static char lookup[256];
    static int first_time=1;
    if (first_time)
    {
        first_time = 0;
        for(int i=0; i<256; i++)
            lookup[i] = -1;
        for(int i='A'; i<='Z'; i++)
            lookup[i] = i-'A';
        for(int i='a'; i<='z'; i++)
            lookup[i] = i-'a' + 26;
        for(int i='0'; i<='9'; i++)
            lookup[i] = i-'0' + 52;
        lookup['+'] = 62;
        lookup['/'] = 63;
    }
    if ((input_size & 3) != 0)
    {
        error("Can't decode base-64 input length %zu", input_size);
        return 0;
    }
    size_t unpadded_size = (input_size/4) * 3;
    if ( unpadded_size > output_size )
    {
        error("Output buffer size %zu is too small to decode %zu into", output_size, input_size);
        return 0;
    }
    // Don't check padding within full quantums
    for (size_t i = 0 ; i < input_size-4 ; i+=4 )
    {
        uint32_t value = (lookup[input[0]] << 18) + (lookup[input[1]] << 12) + (lookup[input[2]] << 6) + lookup[input[3]];
        output[0] = value >> 16;
        output[1] = value >> 8;
        output[2] = value;
        //error("Decoded %c %c %c %c -> %02x %02x %02x", input[0], input[1], input[2], input[3], output[0], output[1], output[2]);
        output += 3;
        input += 4;
    }
    // Handle padding only in last quantum
    if (input[2] == '=') {
        uint32_t value = (lookup[input[0]] << 6) + lookup[input[1]];
        output[0] = value >> 4;
        //error("Decoded %c %c %c %c -> %02x", input[0], input[1], input[2], input[3], output[0]);
        return unpadded_size-2;
    }
    else if (input[3] == '=') {
        uint32_t value = (lookup[input[0]] << 12) + (lookup[input[1]] << 6) + lookup[input[2]];
        output[0] = value >> 10;
        output[1] = value >> 2;
        //error("Decoded %c %c %c %c -> %02x %02x", input[0], input[1], input[2], input[3], output[0], output[1]);
        return unpadded_size-1;
    }
    else
    {
        uint32_t value = (input[0] << 18) + (input[1] << 12) + (input[2]<<6) + input[3];
        output[0] = value >> 16;
        output[1] = value >> 8;
        output[2] = value;
        //error("Decoded %c %c %c %c -> %02x %02x %02x", input[0], input[1], input[2], input[3], output[0], output[1], output[2]);
        return unpadded_size;
    }
}

size_t base64_encode(unsigned char *input, size_t input_size,
                     char *output, size_t output_size)
{
    uint32_t value;
    static char lookup[] = "ABCDEFGHIJKLMNOPQRSTUVWXYZ"
                           "abcdefghijklmnopqrstuvwxyz"
                           "0123456789+/";
    if ((input_size/3+1)*4 >= output_size)
    {
        error("Output buffer for encoding size=%zu is not large enough for %zu-bytes input", output_size, input_size);
        return 0;
    }
    size_t count = 0;
    while (input_size>3)
    {
        value = ((input[0] << 16) + (input[1] << 8) + input[2]) & 0xffffff;
        output[0] = lookup[value >> 18];
        output[1] = lookup[(value >> 12) & 0x3f];
        output[2] = lookup[(value >> 6) & 0x3f];
        output[3] = lookup[value & 0x3f];
        //error("Base-64 encode (%04x) -> %c %c %c %c\n", value, output[0], output[1], output[2], output[3]);
        output += 4;
        input += 3;
        input_size -= 3;
        count += 4;
    }
    switch (input_size)
    {
        case 2:
            value = (input[0] << 10) + (input[1] << 2);
            output[0] = lookup[(value >> 12) & 0x3f];
            output[1] = lookup[(value >> 6) & 0x3f];
            output[2] = lookup[value & 0x3f];
            output[3] = '=';
            //error("Base-64 encode (%06x) -> %c %c %c %c\n", (value>>2)&0xffff, output[0], output[1], output[2], output[3]);
            count += 4;
            break;
        case 1:
            value = input[0] << 4;
            output[0] = lookup[(value >> 6) & 0x3f];
            output[1] = lookup[value & 0x3f];
            output[2] = '=';
            output[3] = '=';
            //error("Base-64 encode (%06x) -> %c %c %c %c\n", value, output[0], output[1], output[2], output[3]);
            count += 4;
            break;
        case 0:
            break;
    }
    return count;
}
