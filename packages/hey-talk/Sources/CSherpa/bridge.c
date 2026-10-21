#include "CSherpa.h"
#include "sherpa-c-api.h"
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <limits.h>

// Only loaded in the disposable inference process. No native model code enters
// the recording/UI process or any existing hey-boss service.
struct HTRecognizer {
    void *library;
    const SherpaOnnxOfflineRecognizer *recognizer;
    __typeof__(&SherpaOnnxDestroyOfflineRecognizer) destroy;
    __typeof__(&SherpaOnnxCreateOfflineStreamWithHotwords) stream;
    __typeof__(&SherpaOnnxDestroyOfflineStream) destroy_stream;
    __typeof__(&SherpaOnnxAcceptWaveformOffline) accept;
    __typeof__(&SherpaOnnxDecodeOfflineStream) decode;
    __typeof__(&SherpaOnnxGetOfflineStreamResult) result;
    __typeof__(&SherpaOnnxDestroyOfflineRecognizerResult) destroy_result;
};
static char last_error[512];
const char *ht_error(void) { return last_error; }
#define LOAD(field, symbol) do { \
    r->field = dlsym(r->library, #symbol); \
    if (!r->field) { snprintf(last_error, sizeof(last_error), "Missing %s", #symbol); goto failed; } \
} while (0)

HTRecognizer *ht_create(const char *library, const char *models, int threads) {
    HTRecognizer *r = calloc(1, sizeof(*r));
    if (!r) return NULL;
    r->library = dlopen(library, RTLD_NOW | RTLD_LOCAL);
    if (!r->library) { snprintf(last_error, sizeof(last_error), "%s", dlerror()); goto failed; }
    __typeof__(&SherpaOnnxCreateOfflineRecognizer) create = dlsym(r->library, "SherpaOnnxCreateOfflineRecognizer");
    if (!create) { snprintf(last_error, sizeof(last_error), "Missing recognizer factory"); goto failed; }
    LOAD(destroy, SherpaOnnxDestroyOfflineRecognizer);
    LOAD(stream, SherpaOnnxCreateOfflineStreamWithHotwords);
    LOAD(destroy_stream, SherpaOnnxDestroyOfflineStream);
    LOAD(accept, SherpaOnnxAcceptWaveformOffline);
    LOAD(decode, SherpaOnnxDecodeOfflineStream);
    LOAD(result, SherpaOnnxGetOfflineStreamResult);
    LOAD(destroy_result, SherpaOnnxDestroyOfflineRecognizerResult);
    char encoder[PATH_MAX], decoder[PATH_MAX], joiner[PATH_MAX], tokens[PATH_MAX], bpe[PATH_MAX];
    snprintf(encoder, sizeof(encoder), "%s/encoder.int8.onnx", models);
    snprintf(decoder, sizeof(decoder), "%s/decoder.int8.onnx", models);
    snprintf(joiner, sizeof(joiner), "%s/joiner.int8.onnx", models);
    snprintf(tokens, sizeof(tokens), "%s/tokens.txt", models);
    snprintf(bpe, sizeof(bpe), "%s/bpe.vocab", models);
    SherpaOnnxOfflineRecognizerConfig config = {0};
    config.feat_config.sample_rate = 16000;
    config.feat_config.feature_dim = 80;
    config.model_config.transducer.encoder = encoder;
    config.model_config.transducer.decoder = decoder;
    config.model_config.transducer.joiner = joiner;
    config.model_config.tokens = tokens;
    config.model_config.num_threads = threads;
    config.model_config.provider = "cpu";
    config.model_config.model_type = "nemo_transducer";
    config.model_config.modeling_unit = "bpe";
    config.model_config.bpe_vocab = bpe;
    config.decoding_method = "modified_beam_search";
    config.max_active_paths = 4;
    config.hotwords_score = 1.5f;
    r->recognizer = create(&config);
    if (!r->recognizer) { snprintf(last_error, sizeof(last_error), "Cannot load Orukeet model"); goto failed; }
    return r;
failed:
    ht_destroy(r);
    return NULL;
}
static char *decode(HTRecognizer *r, const float *samples, int32_t count, const char *hotwords, int json) {
    const SherpaOnnxOfflineStream *s = r->stream(r->recognizer, hotwords);
    if (!s) return NULL;
    r->accept(s, 16000, samples, count);
    r->decode(r->recognizer, s);
    const SherpaOnnxOfflineRecognizerResult *result = r->result(s);
    const char *value = result ? (json ? result->json : result->text) : NULL;
    char *text = value ? strdup(value) : NULL;
    if (result) r->destroy_result(result);
    r->destroy_stream(s);
    return text;
}
char *ht_transcribe(HTRecognizer *r, const float *samples, int32_t count, const char *hotwords) {
    return decode(r, samples, count, hotwords, 0);
}
char *ht_align(HTRecognizer *r, const float *samples, int32_t count) {
    return decode(r, samples, count, "", 1);
}
void ht_free_text(char *text) { free(text); }
void ht_destroy(HTRecognizer *r) {
    if (!r) return;
    if (r->recognizer) r->destroy(r->recognizer);
    if (r->library) dlclose(r->library);
    free(r);
}
