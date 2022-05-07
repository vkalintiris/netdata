#ifndef REPLICATION_H
#define REPLICATION_H

#ifdef __cplusplus
extern "C" {
#endif

void encode_gap_data(BUFFER *buffer);
void decode_gap_data(char *base_64_buf);

#ifdef __cplusplus
}
#endif


#endif /* REPLICATION_H */
