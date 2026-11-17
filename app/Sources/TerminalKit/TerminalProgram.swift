import Foundation

/// What a surface needs of the program running on its terminal.
protocol TerminalProgram: AnyObject, Sendable {
    /// Sends bytes to the program, as if typed.
    func write(_ data: Data)
    func resize(_ size: PTYProcess.Size)
    /// Asks the program to stop the way a closing terminal does.
    func hangUp()
    /// Ends the program without asking, as far as that can be done.
    func endNow()
}

extension PTYProcess: TerminalProgram {}
