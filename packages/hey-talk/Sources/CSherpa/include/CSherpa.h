#ifndef HEY_TALK_SHERPA_H
#define HEY_TALK_SHERPA_H
#include <stdint.h>
typedef struct HTRecognizer HTRecognizer;
HTRecognizer *ht_create(const char *library, const char *models, int threads);
const char *ht_error(void);
char *ht_transcribe(HTRecognizer *recognizer, const float *samples, int32_t count, const char *hotwords);
char *ht_align(HTRecognizer *recognizer, const float *samples, int32_t count);
void ht_free_text(char *text);
void ht_destroy(HTRecognizer *recognizer);
typedef struct HTSpeech HTSpeech;
HTSpeech *ht_speech_create(const char *library, const char *models);
float *ht_speech_generate(HTSpeech *speech, const char *text, int32_t *count, int32_t *rate);
void ht_speech_free(float *samples);
void ht_speech_destroy(HTSpeech *speech);
#endif
