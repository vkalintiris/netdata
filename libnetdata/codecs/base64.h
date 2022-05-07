
// SPDX-License-Identifier: GPL-3.0-or-later
#ifndef NETDATA_CODECS_BASE64_H
#define NETDATA_CODECS_BASE64_H

#include <stddef.h>

size_t base64_decode(unsigned char *input, size_t input_size,
                     unsigned char *output, size_t output_size);

size_t base64_encode(unsigned char *input, size_t input_size,
                     char *output, size_t output_size);

#endif /* NETDATA_CODECS_BASE64_H */
