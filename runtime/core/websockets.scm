;;; WebSocket servers: a port on this machine that other programs connect
;;; to and talk with the editor in text messages, as the extension of a web
;;; browser does.
;;;
;;;   (define server
;;;     (make-websocket-server 4001
;;;       'origins '("moz-extension://")
;;;       'message (lambda (connection text)
;;;                  (websocket-send connection (string-append "got " text)))))
;;;
;;; Servers and connections are numbers, as processes are. The procedures
;;; of a server are called on the editor's thread, in the frame that was
;;; last used.

(define (make-websocket-server port . properties)
  "Listens for WebSocket connections on a port of this machine and returns
the server, a number. Port 0 takes a port that is free, which
`websocket-server-port` then tells. After the port come properties, each a
symbol and a value:

  'open     a procedure called with a new connection, a number, and the
            origin that its request gave, a string or #f
  'message  a procedure called with a connection and each text message
            that arrives on it
  'close    a procedure called with a connection when it has ended,
            whichever side ended it
  'origins  the origins to serve, as a list of the strings that they start
            with: '(\"moz-extension://\") serves the extensions of Firefox.
            A browser says where the code that connects is from, and a
            request from anywhere else is refused. Without this, only
            programs that are not browsers are served, since they give no
            origin
  'reply    a text of JSON to answer with when a program asks the port
            with a plain GET instead of for a WebSocket

The server listens to this machine only. Any web page can ask a browser to
connect to a port of the machine the browser runs on, so serve the origins
that are needed and no more. `websocket-send` and `websocket-close` act on
a connection, and `delete-websocket-server` stops the server. An error if
the port cannot be had, as when another program listens on it."
  (%make-websocket-server port
                          (%property properties 'reply #f)
                          (%property properties 'origins '())
                          (%property properties 'open #f)
                          (%property properties 'message #f)
                          (%property properties 'close #f)))
