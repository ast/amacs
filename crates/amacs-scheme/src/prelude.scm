;;; The part of the standard library that is written in Scheme: mostly the
;;; procedures that call procedures they are given. Loaded into every
;;; interpreter before anything else.

;;; Several values

(define (call-with-values producer consumer)
  "Calls producer, then consumer with the values producer returned."
  (apply consumer (%values->list (producer))))

;;; Leaving and coming back

(define (dynamic-wind before thunk after)
  "Calls before, thunk and after, in that order, and returns what thunk
returned. After is called also if thunk is left by an error or an escape."
  (before)
  (let ((results (%call-with-catch
                  (lambda (raised) (after) (raise raised))
                  (lambda () (%values->list (thunk))))))
    (after)
    (apply values results)))

(define-syntax unwind-protect
  (syntax-rules ()
    ((_ body cleanup ...)
     (dynamic-wind (lambda () #f) (lambda () body) (lambda () cleanup ...)))))

(define (call-with-current-continuation receiver)
  "Calls receiver with an escape procedure: calling that makes this call
return at once, with the arguments it was given. It only works while this
call is in progress; coroutines cover the other uses of continuations."
  (let ((tag (list 'escape)))
    (%call-with-catch
     (lambda (raised)
       (if (and (%escape? raised) (eq? (%escape-tag raised) tag))
           (apply values (%escape-value raised))
           (raise raised)))
     (lambda ()
       (receiver (lambda results (raise (%make-escape tag results))))))))

(define call/cc call-with-current-continuation)

;;; Parameters

(define (make-parameter value . converter)
  "A parameter: a procedure of no arguments that returns a value, which
parameterize changes for the duration of a body. A converter, if given, is
applied to every value the parameter gets."
  (if (pair? converter)
      (%make-parameter ((car converter) value) (car converter))
      (%make-parameter value #f)))

(define (%parameterize bindings thunk)
  (let* ((parameters (%map1 car bindings))
         (values (%map1 (lambda (binding)
                          (let ((convert (%parameter-converter (car binding))))
                            (if convert (convert (cdr binding)) (cdr binding))))
                        bindings))
         (saved (%map1 (lambda (parameter) (parameter)) parameters)))
    (dynamic-wind
     (lambda () (for-each %parameter-set! parameters values))
     thunk
     (lambda () (for-each %parameter-set! parameters saved)))))

;;; Lists

(define (%map1 f items)
  (let loop ((items items) (mapped '()))
    (if (pair? items)
        (loop (cdr items) (cons (f (car items)) mapped))
        (reverse mapped))))

(define (%any-null? lists)
  (and (pair? lists)
       (or (not (pair? (car lists))) (%any-null? (cdr lists)))))

(define (map f first . rest)
  "(map f list ...) A list of what f returns for the items of the lists,
taken together. Stops at the end of the shortest."
  (if (null? rest)
      (%map1 f first)
      (let loop ((lists (cons first rest)) (mapped '()))
        (if (%any-null? lists)
            (reverse mapped)
            (loop (%map1 cdr lists) (cons (apply f (%map1 car lists)) mapped))))))

(define (for-each f first . rest)
  "(for-each f list ...) Calls f with the items of the lists, taken together,
for what it does rather than what it returns."
  (if (null? rest)
      (let loop ((items first))
        (when (pair? items)
          (f (car items))
          (loop (cdr items))))
      (let loop ((lists (cons first rest)))
        (unless (%any-null? lists)
          (apply f (%map1 car lists))
          (loop (%map1 cdr lists))))))

(define (filter keep? items)
  "The items for which keep? returns true."
  (let loop ((items items) (kept '()))
    (cond ((null? items) (reverse kept))
          ((keep? (car items)) (loop (cdr items) (cons (car items) kept)))
          (else (loop (cdr items) kept)))))

(define (remove drop? items)
  "The items for which drop? returns false."
  (filter (lambda (item) (not (drop? item))) items))

(define (partition in? items)
  "Two values: the items for which in? returns true, and the others."
  (let loop ((items items) (in '()) (out '()))
    (cond ((null? items) (values (reverse in) (reverse out)))
          ((in? (car items)) (loop (cdr items) (cons (car items) in) out))
          (else (loop (cdr items) in (cons (car items) out))))))

(define (fold f start items)
  "(fold f start list) Combines the items from the left: f is called with
an item and the result so far."
  (let loop ((items items) (result start))
    (if (null? items)
        result
        (loop (cdr items) (f (car items) result)))))

(define (fold-left f start items)
  "Like fold, but f is called with the result so far and then the item."
  (let loop ((items items) (result start))
    (if (null? items)
        result
        (loop (cdr items) (f result (car items))))))

(define (fold-right f start items)
  "Combines the items from the right: f is called with an item and the
result for the items after it."
  (fold f start (reverse items)))

(define (reduce f default items)
  "Combines the items with f, two at a time. The default is the result for
an empty list."
  (if (null? items)
      default
      (fold f (car items) (cdr items))))

(define (append-map f items)
  "The lists that f returns for the items, joined."
  (apply append (%map1 f items)))

(define (filter-map f items)
  "What f returns for the items, without the #f results."
  (let loop ((items items) (kept '()))
    (if (null? items)
        (reverse kept)
        (let ((mapped (f (car items))))
          (loop (cdr items) (if mapped (cons mapped kept) kept))))))

(define (find-tail wanted? items)
  "The part of the list that starts with the first item for which wanted?
returns true, or #f."
  (let loop ((items items))
    (cond ((null? items) #f)
          ((wanted? (car items)) items)
          (else (loop (cdr items))))))

(define (find wanted? items)
  "The first item for which wanted? returns true, or #f."
  (let ((tail (find-tail wanted? items)))
    (and tail (car tail))))

(define (list-index wanted? items)
  "The position of the first item for which wanted? returns true, or #f."
  (let loop ((items items) (index 0))
    (cond ((null? items) #f)
          ((wanted? (car items)) index)
          (else (loop (cdr items) (+ index 1))))))

(define (any test? items)
  "The first true result of test? on the items, or #f."
  (let loop ((items items))
    (and (pair? items)
         (or (test? (car items)) (loop (cdr items))))))

(define (every test? items)
  "#f if test? returns #f for any item; otherwise its last result."
  (let loop ((items items) (last #t))
    (if (null? items)
        last
        (let ((result (test? (car items))))
          (and result (loop (cdr items) result))))))

(define (count wanted? items)
  "How many items wanted? returns true for."
  (fold (lambda (item total) (if (wanted? item) (+ total 1) total)) 0 items))

(define (delete unwanted items)
  "The items that are not equal? to unwanted."
  (filter (lambda (item) (not (equal? item unwanted))) items))

(define (delete-duplicates items)
  "The items without those that are equal? to an earlier one."
  (let loop ((items items) (seen '()))
    (cond ((null? items) (reverse seen))
          ((%member (car items) seen) (loop (cdr items) seen))
          (else (loop (cdr items) (cons (car items) seen))))))

(define (member wanted items . same?)
  "(member x list [same?]) The part of the list that starts with x, or #f.
Compares with equal? unless another test is given."
  (if (null? same?)
      (%member wanted items)
      (find-tail (lambda (item) ((car same?) wanted item)) items)))

(define (assoc key entries . same?)
  "(assoc key alist [same?]) The first pair in the list whose car is the
key, or #f. Compares with equal? unless another test is given."
  (if (null? same?)
      (%assoc key entries)
      (find (lambda (entry) ((car same?) key (car entry))) entries)))

(define (list-set! items index value)
  "Changes item number index of a list."
  (set-car! (list-tail items index) value))

;;; Sorting

(define (%merge less? left right)
  (let loop ((left left) (right right) (merged '()))
    (cond ((null? left) (append (reverse merged) right))
          ((null? right) (append (reverse merged) left))
          ((less? (car right) (car left))
           (loop left (cdr right) (cons (car right) merged)))
          (else (loop (cdr left) right (cons (car left) merged))))))

(define (list-sort less? items)
  "The items in order, as less? says. Items that are neither less than the
other keep their order."
  (let sort ((items items) (count (length items)))
    (cond ((= count 0) '())
          ((= count 1) (list (car items)))
          (else
           (let ((half (quotient count 2)))
             (%merge less?
                     (sort items half)
                     (sort (list-tail items half) (- count half))))))))

(define (sort items less?)
  "(sort list less?) The items of a list or vector in order, as less? says."
  (cond ((procedure? items) (sort less? items))
        ((vector? items) (list->vector (list-sort less? (vector->list items))))
        (else (list-sort less? items))))

;;; Vectors and strings

(define (vector-map f first . rest)
  "A vector of what f returns for the items of the vectors, taken together."
  (list->vector (apply map f (vector->list first) (%map1 vector->list rest))))

(define (vector-for-each f first . rest)
  "Calls f with the items of the vectors, taken together."
  (apply for-each f (vector->list first) (%map1 vector->list rest)))

(define (string-map f first . rest)
  "A string of the chars f returns for the chars of the strings."
  (list->string (apply map f (string->list first) (%map1 string->list rest))))

(define (string-for-each f first . rest)
  "Calls f with the chars of the strings, taken together."
  (apply for-each f (string->list first) (%map1 string->list rest)))

(define (string-count string wanted?)
  "How many chars of the string wanted? returns true for."
  (count wanted? (string->list string)))

;;; Hash tables

(define (hash-table-update! table key update . default)
  "Sets the value for a key to what update returns for the present value.
Default, a procedure of no arguments, gives the value if there is none."
  (hash-table-set!
   table key
   (update (cond ((hash-table-contains? table key) (hash-table-ref table key))
                 ((pair? default) ((car default)))
                 (else (error "hash-table-update!: no value for the key" key))))))

(define (hash-table-update!/default table key update default)
  "Sets the value for a key to what update returns for the present value,
or for default if there is none."
  (hash-table-set! table key (update (hash-table-ref table key default))))

(define (hash-table-walk table f)
  "Calls f with each key and its value."
  (for-each (lambda (entry) (f (car entry) (cdr entry)))
            (hash-table->alist table)))

(define (alist->hash-table entries)
  "A hash table of the (key . value) pairs in a list."
  (let ((table (make-hash-table)))
    (for-each (lambda (entry) (hash-table-set! table (car entry) (cdr entry)))
              entries)
    table))

;;; Numbers

(define (floor/ n d)
  "Two values: n divided by d, rounded down, and what is left."
  (values (floor-quotient n d) (floor-remainder n d)))

(define (truncate/ n d)
  "Two values: n divided by d, rounded towards zero, and what is left."
  (values (truncate-quotient n d) (truncate-remainder n d)))

;;; Promises

(define-record-type promise
  (%make-promise done? value)
  %promise?
  (done? %promise-done? %promise-done!)
  (value %promise-value %promise-value!))

(define-syntax delay-force
  (syntax-rules ()
    ((_ expression) (%make-promise #f (lambda () expression)))))

(define-syntax delay
  (syntax-rules ()
    ((_ expression) (delay-force (%make-promise #t expression)))))

(define (promise? x)
  "Whether x is a promise, as made by delay."
  (%promise? x))

(define (make-promise value)
  "A promise that already has its value."
  (if (promise? value) value (%make-promise #t value)))

(define (force promise)
  "The value of a promise, which is worked out the first time it is asked
for. Anything else is returned as it is."
  (cond ((not (promise? promise)) promise)
        ((%promise-done? promise) (%promise-value promise))
        (else
         (let ((next ((%promise-value promise))))
           (unless (%promise-done? promise)
             (%promise-done! promise (%promise-done? next))
             (%promise-value! promise (%promise-value next)))
           (force promise)))))

;;; More syntax

(define-syntax case-lambda
  (syntax-rules ()
    ((_ (parameters body0 ...) ...)
     (lambda arguments
       (let ((given (length arguments)))
         (letrec-syntax
             ((clauses
               (syntax-rules ::: ()
                 ((_)
                  (error "case-lambda: no clause takes this many arguments" given))
                 ((_ ((parameter :::) . body) . more)
                  (if (= given (length '(parameter :::)))
                      (apply (lambda (parameter :::) . body) arguments)
                      (clauses . more)))
                 ((_ ((parameter ::: . tail) . body) . more)
                  (if (>= given (length '(parameter :::)))
                      (apply (lambda (parameter ::: . tail) . body) arguments)
                      (clauses . more))))))
           (clauses (parameters body0 ...) ...)))))))

(define-syntax define-syntax-rule
  (syntax-rules ()
    ((_ (name . pattern) template)
     (define-syntax name
       (syntax-rules ()
         ((_ . pattern) template))))))

(define-syntax while
  (syntax-rules ()
    ((_ test body ...)
     (let loop ()
       (when test
         body ...
         (loop))))))

(define-syntax assert
  (syntax-rules ()
    ((_ expression)
     (unless expression
       (error "assertion failed:" 'expression)))))

;;; Output

(define (with-output-to-string thunk)
  "Calls thunk and returns what it wrote with display and its like, as a
string, instead of letting it out."
  (let ((mark (%output-mark)))
    (thunk)
    (%output-take mark)))

(define (features)
  "What this Scheme provides."
  '(r7rs amacs full-unicode))

(define (identity x)
  "Returns its argument."
  x)
