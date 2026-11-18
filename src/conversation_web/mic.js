// Safari may run the graph at the hardware rate. Emit 80 ms of 16 kHz mono PCM.
class ConversationMic extends AudioWorkletProcessor {
  constructor() {
    super(); this.chunk = new Int16Array(1280); this.used = this.phase = this.sum = this.n = 0;
    this.port.onmessage = ({data: muted}) => {
      if (muted === null) { this.closed = true; return; }
      this.muted = muted;
      if (muted) this.used = this.phase = this.sum = this.n = 0;
    };
  }
  process(inputs) {
    if (this.closed) { this.port.postMessage(null); return false; }
    if (this.muted) return true;
    for (const sample of inputs[0][0] || []) {
      this.sum += sample; this.n++; this.phase += 16000;
      if (this.phase < sampleRate) continue;
      const value = Math.round(32767 * Math.max(-1, Math.min(1, this.sum / this.n)));
      this.sum = this.n = 0;
      do {
        this.phase -= sampleRate;
        this.chunk[this.used++] = value;
        if (this.used === this.chunk.length) {
          this.port.postMessage(this.chunk.buffer, [this.chunk.buffer]);
          this.chunk = new Int16Array(1280); this.used = 0;
        }
      } while (this.phase >= sampleRate);
    }
    return true;
  }
}
registerProcessor('conversation-mic', ConversationMic);
