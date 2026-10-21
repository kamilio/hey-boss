#include "CSherpa.h"
#include "sherpa-c-api.h"
#include <dlfcn.h>
#include <stdlib.h>
#include <stdio.h>
#include <string.h>
#include <limits.h>

struct HTSpeech {
    void *library;
    const SherpaOnnxOfflineTts *tts;
    __typeof__(&SherpaOnnxDestroyOfflineTts) destroy;
    __typeof__(&SherpaOnnxOfflineTtsGenerateWithConfig) generate;
    __typeof__(&SherpaOnnxDestroyOfflineTtsGeneratedAudio) free_audio;
};

HTSpeech *ht_speech_create(const char *library, const char *models) {
    HTSpeech *s = calloc(1, sizeof(*s));
    if (!s) return NULL;
    s->library = dlopen(library, RTLD_NOW | RTLD_LOCAL);
    if (!s->library) goto failed;
    __typeof__(&SherpaOnnxCreateOfflineTts) create = dlsym(s->library, "SherpaOnnxCreateOfflineTts");
    s->destroy = dlsym(s->library, "SherpaOnnxDestroyOfflineTts");
    s->generate = dlsym(s->library, "SherpaOnnxOfflineTtsGenerateWithConfig");
    s->free_audio = dlsym(s->library, "SherpaOnnxDestroyOfflineTtsGeneratedAudio");
    if (!create || !s->destroy || !s->generate || !s->free_audio) goto failed;
    char paths[7][PATH_MAX];
    const char *names[] = {"duration_predictor.int8.onnx", "text_encoder.int8.onnx",
        "vector_estimator.int8.onnx", "vocoder.int8.onnx", "tts.json", "unicode_indexer.bin", "voice.bin"};
    for (int i = 0; i < 7; ++i) snprintf(paths[i], sizeof(paths[i]), "%s/%s", models, names[i]);
    SherpaOnnxOfflineTtsConfig config = {0};
    config.model.supertonic = (SherpaOnnxOfflineTtsSupertonicModelConfig){
        paths[0], paths[1], paths[2], paths[3], paths[4], paths[5], paths[6]};
    config.model.num_threads = 2;
    config.model.provider = "cpu";
    config.max_num_sentences = 1;
    config.silence_scale = 0.2f;
    s->tts = create(&config);
    if (!s->tts) goto failed;
    return s;
failed:
    ht_speech_destroy(s);
    return NULL;
}

float *ht_speech_generate(HTSpeech *s, const char *text, int32_t *count, int32_t *rate) {
    SherpaOnnxGenerationConfig config = {0};
    config.speed = 1; config.sid = 6; config.num_steps = 5;
    config.extra = "{\"lang\":\"en\"}";
    const SherpaOnnxGeneratedAudio *audio = s->generate(s->tts, text, &config, NULL, NULL);
    if (!audio) return NULL;
    float *samples = NULL;
    if (audio->n > 0 && audio->n <= 44100 * 120 && audio->sample_rate == 44100 && audio->samples) {
        samples = malloc((size_t)audio->n * sizeof(float));
        if (samples) {
            memcpy(samples, audio->samples, (size_t)audio->n * sizeof(float));
            *count = audio->n; *rate = audio->sample_rate;
        }
    }
    s->free_audio(audio);
    return samples;
}
void ht_speech_free(float *samples) { free(samples); }
void ht_speech_destroy(HTSpeech *s) {
    if (!s) return;
    if (s->tts) s->destroy(s->tts);
    if (s->library) dlclose(s->library);
    free(s);
}
