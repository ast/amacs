;;; What r7rs-tests.scm needs of the (chibi test) library, with a count of
;;; the outcomes instead of printed ones.

(define %passed 0)
(define %failures '())

(define (test-begin . name) #f)
(define (test-end . name) #f)

(define (%fail! form . details)
  (set! %failures (cons (cons form details) %failures)))

(define (%close? expected actual)
  (or (equal? expected actual)
      (and (number? expected) (number? actual)
           (or (inexact? expected) (inexact? actual))
           (or (and (nan? expected) (nan? actual))
               (<= (abs (- expected actual))
                   (* 1e-10 (max 1 (abs expected))))))
      (and (pair? expected) (pair? actual)
           (%close? (car expected) (car actual))
           (%close? (cdr expected) (cdr actual)))
      (and (vector? expected) (vector? actual)
           (%close? (vector->list expected) (vector->list actual)))))

(define (%check form expected actual)
  (guard (raised (#t (%fail! form 'raised
                             (if (error-object? raised)
                                 (error-object->string raised)
                                 raised))))
    (let ((expected (expected)) (actual (actual)))
      (if (%close? expected actual)
          (set! %passed (+ %passed 1))
          (%fail! form 'expected expected 'got actual)))))

(define-syntax test
  (syntax-rules ()
    ((_ expected expression)
     (%check 'expression (lambda () expected) (lambda () expression)))
    ((_ name expected expression)
     (test expected expression))))

(define-syntax test-values
  (syntax-rules ()
    ((_ expected expression)
     (test (call-with-values (lambda () expected) list)
           (call-with-values (lambda () expression) list)))))

(define-syntax test-assert
  (syntax-rules ()
    ((_ expression) (test #t (if expression #t #f)))
    ((_ name expression) (test-assert expression))))

(define-syntax test-error
  (syntax-rules ()
    ((_ expression)
     (if (guard (raised (#t #t)) expression #f)
         (set! %passed (+ %passed 1))
         (%fail! 'expression 'expected 'an-error)))
    ((_ name expression) (test-error expression))))
