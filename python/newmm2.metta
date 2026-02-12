(home (animal (person (male (1)))))
(home (animal (person (male (1 value)))))
(home (animal (person (male (2)))))
(home (animal (person (female (1)))))
(home (animal (a (person (female (2))))))
(home (life (person (female (2)))))

(aunt (animal (person (male (1)))))

(foo 1) (foo 2) (foo 3)  
(bar x) (bar y) (bar 1) 
(baz P) (baz Q) (baz R)  

(exec 0 (, (bar $x) ) (O (ws 20 (bar $x))) ) 
(exec 0 (, (foo $x) ) (O (ws 4 (foo $x))) ) 
(exec 0 (, (bar $x) ) (O (ws 3 (bar $x))) ) 
