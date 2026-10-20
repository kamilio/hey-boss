#ifndef HEY_TALK_SHERPA_H
#define HEY_TALK_SHERPA_H
#include <stdint.h>
typedef struct HTRecognizer HTRecognizer;
HTRecognizer *ht_create(const char *library, const char *models, int threads);
const char *ht_error(void);
char *ht_transcribe(HTRecognizer *recognizer, const float *samples, int32_t count, const char *hotwords);
void ht_free_text(char *text);
void ht_destroy(HTRecognizer *recognizer);
#endif
