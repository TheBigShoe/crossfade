import AVFoundation
import CrossfadeFFI
import Foundation

/// The audio session's events: an interruption (a call, an alarm) pauses and may resume;
/// losing the output device (headphones unplugged) pauses, as Apple's guidelines ask; an
/// engine configuration change (a new route at another rate) rebuilds the player.
extension PreviewPlayer {
    func observeSession() {
        let center = NotificationCenter.default
        let session = AVAudioSession.sharedInstance()
        observers.append(center.addObserver(
            forName: AVAudioSession.interruptionNotification, object: session, queue: .main
        ) { [weak self] note in
            let type = note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt ?? 0
            let options = note.userInfo?[AVAudioSessionInterruptionOptionKey] as? UInt ?? 0
            Task { @MainActor [weak self] in
                self?.interrupted(type: type, options: options)
            }
        })
        observers.append(center.addObserver(
            forName: AVAudioSession.routeChangeNotification, object: session, queue: .main
        ) { [weak self] note in
            let reason = note.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt ?? 0
            Task { @MainActor [weak self] in
                self?.routeChanged(reason: reason)
            }
        })
        observers.append(center.addObserver(
            forName: .AVAudioEngineConfigurationChange, object: nil, queue: .main
        ) { [weak self] _ in
            Task { @MainActor [weak self] in
                await self?.rebuild()
            }
        })
    }

    func stopObservingSession() {
        for token in observers {
            NotificationCenter.default.removeObserver(token)
        }
        observers.removeAll()
    }

    private func interrupted(type: UInt, options: UInt) {
        switch AVAudioSession.InterruptionType(rawValue: type) {
        case .began:
            resumeAfterInterruption = state?.state == "playing"
            try? pause()
        case .ended:
            let resume = AVAudioSession.InterruptionOptions(rawValue: options).contains(.shouldResume)
            if resume && resumeAfterInterruption {
                try? play()
            }
            resumeAfterInterruption = false
        default:
            break
        }
    }

    private func routeChanged(reason: UInt) {
        if AVAudioSession.RouteChangeReason(rawValue: reason) == .oldDeviceUnavailable {
            try? pause()
        }
    }
}
