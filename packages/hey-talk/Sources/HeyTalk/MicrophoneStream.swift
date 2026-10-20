import Foundation
import AVFoundation
import CoreAudio
import TalkCore

protocol MicrophoneStream: AnyObject {
    func start() throws
    func stop()
}

enum MicrophoneAccessError: LocalizedError {
    case denied
    var errorDescription: String? { "Microphone access is required. Run hey-talk permissions microphone." }
}

private final class CaptureReceiver: NSObject, AVCaptureAudioDataOutputSampleBufferDelegate {
    let receive: (CMSampleBuffer) -> Void
    init(receive: @escaping (CMSampleBuffer) -> Void) { self.receive = receive }
    func captureOutput(_ output: AVCaptureOutput, didOutput sampleBuffer: CMSampleBuffer, from connection: AVCaptureConnection) {
        receive(sampleBuffer)
    }
}

final class SystemMicrophoneStream: MicrophoneStream {
    private let session = AVCaptureSession()
    private let output = AVCaptureAudioDataOutput()
    private let receiver: CaptureReceiver
    private var observers: [NSObjectProtocol] = []

    init(device: InputDevice, queue: DispatchQueue, receive: @escaping (CMSampleBuffer) -> Void,
         interrupted: @escaping (String) -> Void) throws {
        guard AVCaptureDevice.authorizationStatus(for: .audio) == .authorized else { throw MicrophoneAccessError.denied }
        let devices = AVCaptureDevice.DiscoverySession(deviceTypes: [.microphone, .external], mediaType: .audio, position: .unspecified).devices
        guard let device = devices.first(where: { $0.uniqueID == device.uid }) else {
            throw TalkError("Microphone unavailable. Check the system input in Sound settings.")
        }
        let input = try AVCaptureDeviceInput(device: device)
        receiver = CaptureReceiver(receive: receive)
        output.audioSettings = [AVFormatIDKey: kAudioFormatLinearPCM, AVSampleRateKey: 16_000,
                                AVNumberOfChannelsKey: 1, AVLinearPCMBitDepthKey: 32,
                                AVLinearPCMIsFloatKey: true, AVLinearPCMIsNonInterleaved: false]
        output.setSampleBufferDelegate(receiver, queue: queue)
        // Keep this capture-only: an output graph can hold a stale Bluetooth format.
        session.beginConfiguration()
        guard session.canAddInput(input), session.canAddOutput(output) else {
            session.commitConfiguration(); throw TalkError("Cannot configure microphone capture")
        }
        session.addInput(input); session.addOutput(output); session.commitConfiguration()
        for name in [AVCaptureSession.runtimeErrorNotification, AVCaptureSession.wasInterruptedNotification] {
            observers.append(NotificationCenter.default.addObserver(forName: name, object: session, queue: nil) { notification in
                let error = notification.userInfo?[AVCaptureSessionErrorKey] as? Error
                interrupted(error?.localizedDescription ?? "Microphone capture was interrupted.")
            })
        }
    }
    func start() throws {
        session.startRunning()
        guard session.isRunning else { throw TalkError("Microphone could not start") }
    }
    func stop() {
        observers.forEach(NotificationCenter.default.removeObserver); observers = []
        output.setSampleBufferDelegate(nil, queue: nil)
        session.stopRunning()
    }
    deinit { stop() }
}

/// Listen while recording; the watchdog also reconciles routes if a driver misses a notification.
final class AudioInputObserver {
    private let queue: DispatchQueue
    private let listener: AudioObjectPropertyListenerBlock
    private var addresses: [AudioObjectPropertyAddress] = []
    init(queue: DispatchQueue, changed: @escaping () -> Void) {
        self.queue = queue
        listener = { _, _ in changed() }
        for selector in [kAudioHardwarePropertyDefaultInputDevice, kAudioHardwarePropertyDevices] {
            var address = AudioObjectPropertyAddress(mSelector: selector, mScope: kAudioObjectPropertyScopeGlobal,
                                                     mElement: kAudioObjectPropertyElementMain)
            if AudioObjectAddPropertyListenerBlock(AudioObjectID(kAudioObjectSystemObject), &address, queue, listener) == noErr {
                addresses.append(address)
            }
        }
    }
    deinit {
        for var address in addresses {
            AudioObjectRemovePropertyListenerBlock(AudioObjectID(kAudioObjectSystemObject), &address, queue, listener)
        }
    }
}
